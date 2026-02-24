use gpui::*;
use smol::Timer;
use std::time::Duration;

use super::agent::spawn_agent;
use super::git::{get_diff_stat, get_snapshot};
use super::status_parser::{build_prompt, check_done_sentinel, ensure_agent_md, parse_issue_overrides, parse_status};
use super::types::KruhState;
use super::KruhPane;

/// Spawn the async iteration loop as a GPUI task.
pub fn start_loop(cx: &mut Context<KruhPane>) -> Task<()> {
    cx.spawn(async move |this: WeakEntity<KruhPane>, cx| {
        run_loop(this, cx).await;
    })
}

async fn run_loop(this: WeakEntity<KruhPane>, cx: &mut AsyncApp) {
    loop {
        // 1. Check quit
        let should_quit = this.update(cx, |pane, _cx| pane.quit_requested).unwrap_or(true);
        if should_quit {
            break;
        }

        // 2. Get config and project path
        let (base_config, project_path, iteration) =
            match this.update(cx, |pane, _cx| {
                (
                    pane.config.clone(),
                    pane.project_path.clone(),
                    pane.iteration,
                )
            }) {
                Ok(v) => v,
                Err(_) => break, // Entity dropped
            };

        // 3. Check max iterations (using base config — issue overrides may lower it)
        if iteration >= base_config.max_iterations {
            let _ = this.update(cx, |pane, cx| {
                pane.state = KruhState::Completed;
                pane.add_output("Max iterations reached.", false);
                cx.notify();
            });
            break;
        }

        // 4. Parse STATUS.md to find pending issues
        let docs_dir = base_config.docs_dir.clone();
        let progress = smol::unblock(move || parse_status(&docs_dir)).await;

        let first_pending_number = match &progress {
            Ok(p) => p.pending_refs.first().map(|r| r.number.clone()),
            Err(_) => None,
        };

        // 5. Parse per-issue overrides from the first pending issue's frontmatter
        let docs_dir_for_overrides = base_config.docs_dir.clone();
        let issue_num = first_pending_number.clone().unwrap_or_default();
        let overrides = smol::unblock(move || {
            parse_issue_overrides(&docs_dir_for_overrides, &issue_num)
        }).await;
        let config = base_config.with_overrides(&overrides);

        // Check max_iterations with overrides applied
        if iteration >= config.max_iterations {
            let _ = this.update(cx, |pane, cx| {
                pane.state = KruhState::Completed;
                pane.add_output("Max iterations reached.", false);
                cx.notify();
            });
            break;
        }

        match progress {
            Ok(progress) => {
                let all_done = progress.pending == 0 && progress.total > 0;
                let _ = this.update(cx, |pane, cx| {
                    pane.progress = progress;
                    cx.notify();
                });
                if all_done {
                    let _ = this.update(cx, |pane, cx| {
                        pane.state = KruhState::Completed;
                        pane.add_output("All tasks completed!", false);
                        cx.notify();
                    });
                    break;
                }
            }
            Err(e) => {
                let _ = this.update(cx, |pane, cx| {
                    pane.add_output(&format!("Failed to parse STATUS.md: {}", e), true);
                    cx.notify();
                });
                // Continue anyway — agent might create STATUS.md
            }
        }

        // 5. Check pause — spin until unpaused or quit
        loop {
            let is_paused = this.update(cx, |pane, _| pane.paused).unwrap_or(false);
            if !is_paused {
                break;
            }
            let _ = this.update(cx, |pane, cx| {
                pane.state = KruhState::Paused;
                cx.notify();
            });
            Timer::after(Duration::from_millis(200)).await;
            let should_quit = this.update(cx, |pane, _| pane.quit_requested).unwrap_or(true);
            if should_quit {
                let _ = this.update(cx, |pane, cx| {
                    pane.state = KruhState::Completed;
                    cx.notify();
                });
                return;
            }
        }

        // 6a. Ensure AGENT.md exists in plans dir
        let plans_dir_for_agent = config.plans_dir.clone();
        if let Err(e) = smol::unblock(move || ensure_agent_md(&plans_dir_for_agent)).await {
            let _ = this.update(cx, |pane, cx| {
                pane.add_output(&format!("Warning: could not create AGENT.md: {}", e), true);
                cx.notify();
            });
        }

        // 6b. Build prompt
        let docs_dir = config.docs_dir.clone();
        let plans_dir_for_prompt = config.plans_dir.clone();
        let prompt = match smol::unblock(move || build_prompt(&docs_dir, &plans_dir_for_prompt)).await {
            Ok(p) => p,
            Err(e) => {
                let _ = this.update(cx, |pane, cx| {
                    pane.add_output(&format!("Failed to build prompt: {}", e), true);
                    pane.state = KruhState::Completed;
                    cx.notify();
                });
                break;
            }
        };

        // 7. Update state to Running, increment iteration
        let _ = this.update(cx, |pane, cx| {
            pane.state = KruhState::Running;
            pane.iteration += 1;
            pane.add_output(&format!("--- Iteration {} ---", pane.iteration), false);
            cx.notify();
        });

        // 8. Git snapshot before
        let pp = project_path.clone();
        let before_snapshot = smol::unblock(move || get_snapshot(&pp)).await;

        // 9. Spawn agent
        let agent_result = {
            let cfg = config.clone();
            let pp = project_path.clone();
            let p = prompt.clone();
            smol::unblock(move || spawn_agent(&cfg, &pp, &p)).await
        };

        let mut handle = match agent_result {
            Ok(h) => h,
            Err(e) => {
                let _ = this.update(cx, |pane, cx| {
                    pane.add_output(&format!("Failed to spawn agent: {}", e), true);
                    pane.fail_count += 1;
                    cx.notify();
                });
                continue;
            }
        };

        // 10. Stream output lines
        let mut full_output = String::new();
        loop {
            // Check quit/skip
            let (should_quit, should_skip) = this
                .update(cx, |pane, _| (pane.quit_requested, pane.skip_requested))
                .unwrap_or((true, false));

            if should_quit || should_skip {
                handle.kill();
                if should_skip {
                    let _ = this.update(cx, |pane, cx| {
                        pane.skip_requested = false;
                        pane.add_output("Skipped.", false);
                        cx.notify();
                    });
                }
                if should_quit {
                    let _ = this.update(cx, |pane, cx| {
                        pane.state = KruhState::Completed;
                        cx.notify();
                    });
                    return;
                }
                break;
            }

            // Try to receive stdout lines (non-blocking)
            match handle.stdout_receiver.try_recv() {
                Ok(line) => {
                    full_output.push_str(&line);
                    full_output.push('\n');
                    let _ = this.update(cx, |pane, cx| {
                        pane.add_output(&line, false);
                        cx.notify();
                    });
                    continue; // Drain all available lines before sleeping
                }
                Err(async_channel::TryRecvError::Empty) => {}
                Err(async_channel::TryRecvError::Closed) => break,
            }

            // Also drain stderr
            while let Ok(line) = handle.stderr_receiver.try_recv() {
                let _ = this.update(cx, |pane, cx| {
                    pane.add_output(&line, true);
                    cx.notify();
                });
            }

            // Check if process exited
            match handle.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(_) => break,
            }

            // Small sleep to avoid busy-waiting
            Timer::after(Duration::from_millis(50)).await;
        }

        // 11. Wait for process to fully exit
        let exit_code = loop {
            match handle.try_wait() {
                Ok(Some(code)) => break code,
                Ok(None) => { Timer::after(Duration::from_millis(100)).await; }
                Err(_) => break -1,
            }
        };

        // Drain remaining stdout
        while let Ok(line) = handle.stdout_receiver.try_recv() {
            full_output.push_str(&line);
            full_output.push('\n');
            let _ = this.update(cx, |pane, cx| {
                pane.add_output(&line, false);
                cx.notify();
            });
        }

        // 12. Check done sentinel
        let is_done = check_done_sentinel(&full_output);

        // 13. Git diff
        let diff_stat = {
            let pp = project_path.clone();
            let before = before_snapshot.clone();
            smol::unblock(move || {
                let after = get_snapshot(&pp);
                match (before, after) {
                    (Some(b), Some(a)) if b != a => get_diff_stat(&pp, &b, &a),
                    _ => None,
                }
            })
            .await
        };

        // 14. Update state
        let _ = this.update(cx, |pane, cx| {
            pane.diff_stat = diff_stat;
            if exit_code == 0 {
                pane.pass_count += 1;
            } else {
                pane.fail_count += 1;
            }
            pane.add_output(
                &format!("Agent exited with code {}", exit_code),
                exit_code != 0,
            );
            cx.notify();
        });

        if is_done {
            let _ = this.update(cx, |pane, cx| {
                pane.state = KruhState::Completed;
                pane.add_output("Agent signaled completion.", false);
                cx.notify();
            });
            break;
        }

        // 15. Step mode: wait for user advancement
        let step_mode = this.update(cx, |pane, _| pane.step_mode).unwrap_or(false);
        if step_mode {
            let _ = this.update(cx, |pane, cx| {
                pane.state = KruhState::WaitingForStep;
                cx.notify();
            });
            loop {
                let (quit, skip, step_advance) = this
                    .update(cx, |pane, _| {
                        (
                            pane.quit_requested,
                            pane.skip_requested,
                            pane.step_advance_requested,
                        )
                    })
                    .unwrap_or((true, false, false));
                if quit || skip || step_advance {
                    let _ = this.update(cx, |pane, _| {
                        pane.step_advance_requested = false;
                    });
                    if quit {
                        let _ = this.update(cx, |pane, cx| {
                            pane.state = KruhState::Completed;
                            cx.notify();
                        });
                        return;
                    }
                    break;
                }
                Timer::after(Duration::from_millis(200)).await;
            }
        } else {
            // 16. Sleep between iterations
            let sleep_secs = config.sleep_secs;
            let _ = this.update(cx, |pane, cx| {
                pane.add_output(&format!("Sleeping {}s...", sleep_secs), false);
                cx.notify();
            });
            Timer::after(Duration::from_secs(sleep_secs)).await;
        }
    }

    // Final cleanup
    let _ = this.update(cx, |pane, cx| {
        if pane.state != KruhState::Completed {
            pane.state = KruhState::Completed;
        }
        pane.agent_handle = None;
        cx.notify();
    });
}
