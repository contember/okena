use gpui::*;
use smol::Timer;
use std::time::Duration;

use super::agent::spawn_agent;
use super::git::{get_diff_stat, get_snapshot};
use super::status_parser::{build_prompt, check_done_sentinel, ensure_agent_md, find_issue_file_path, iso_now, parse_issue_overrides, parse_status, update_issue_frontmatter};
use super::types::LoopState;
use super::KruhPane;

/// Spawn the async iteration loop as a GPUI task for a specific loop instance.
pub fn start_loop(loop_id: usize, cx: &mut Context<KruhPane>) -> Task<()> {
    cx.spawn(async move |this: WeakEntity<KruhPane>, cx| {
        run_loop(loop_id, this, cx).await;
    })
}

async fn run_loop(loop_id: usize, this: WeakEntity<KruhPane>, cx: &mut AsyncApp) {
    // Add initial output
    let _ = this.update(cx, |pane, cx| {
        pane.add_loop_output(loop_id, "Loop started.", false);
        cx.notify();
    });

    loop {
        // 1. Check quit
        let should_quit = this.update(cx, |pane, _cx| {
            pane.loop_ref(loop_id).map(|l| l.quit_requested).unwrap_or(true)
        }).unwrap_or(true);
        if should_quit {
            break;
        }

        // 2. Get config and project path
        let (config, project_path, iteration) =
            match this.update(cx, |pane, _cx| {
                let l = pane.loop_ref(loop_id)?;
                Some((
                    l.config.clone(),
                    pane.project_path.clone(),
                    l.iteration,
                ))
            }) {
                Ok(Some(v)) => v,
                _ => break, // Entity dropped or loop not found
            };

        // 3. Check max iterations (using base config — issue overrides may lower it)
        if iteration >= config.max_iterations {
            let _ = this.update(cx, |pane, cx| {
                if let Some(l) = pane.loop_mut(loop_id) {
                    l.state = LoopState::Completed;
                    l.loop_phase = Default::default();
                    l.add_output("Max iterations reached.", false);
                }
                cx.notify();
            });
            break;
        }

        // 4. Parse STATUS.md to find pending issues
        let _ = this.update(cx, |pane, cx| {
            if let Some(l) = pane.loop_mut(loop_id) {
                l.loop_phase = super::types::LoopPhase::ParsingStatus;
            }
            cx.notify();
        });
        let docs_dir = config.docs_dir.clone();
        let progress = smol::unblock(move || parse_status(&docs_dir)).await;

        let first_pending_number = match &progress {
            Ok(p) => p.pending_refs.first().map(|r| r.number.clone()),
            Err(_) => None,
        };

        // 5. Parse per-issue overrides from the first pending issue's frontmatter
        let docs_dir_for_overrides = config.docs_dir.clone();
        let issue_num = first_pending_number.clone().unwrap_or_default();
        let overrides = smol::unblock(move || {
            parse_issue_overrides(&docs_dir_for_overrides, &issue_num)
        }).await;
        let effective_config = config.with_overrides(&overrides);

        // Check max_iterations with overrides applied
        if iteration >= effective_config.max_iterations {
            let _ = this.update(cx, |pane, cx| {
                if let Some(l) = pane.loop_mut(loop_id) {
                    l.state = LoopState::Completed;
                    l.loop_phase = Default::default();
                    l.add_output("Max iterations reached.", false);
                }
                cx.notify();
            });
            break;
        }

        match progress {
            Ok(progress) => {
                let all_done = progress.pending == 0 && progress.total > 0;
                let issue_name = progress.pending_refs.first().map(|r| {
                    format!("#{} \u{2014} {}", r.number, r.name)
                });
                let _ = this.update(cx, |pane, cx| {
                    if let Some(l) = pane.loop_mut(loop_id) {
                        l.progress = progress;
                        l.current_issue_name = issue_name;
                    }
                    cx.notify();
                });
                if all_done {
                    let _ = this.update(cx, |pane, cx| {
                        if let Some(l) = pane.loop_mut(loop_id) {
                            l.state = LoopState::Completed;
                            l.loop_phase = Default::default();
                            l.add_output("All tasks completed!", false);
                        }
                        cx.notify();
                    });
                    break;
                }
            }
            Err(e) => {
                let _ = this.update(cx, |pane, cx| {
                    pane.add_loop_output(loop_id, &format!("Failed to parse STATUS.md: {}", e), true);
                    cx.notify();
                });
                // Continue anyway — agent might create STATUS.md
            }
        }

        // 5. Check pause — spin until unpaused or quit
        loop {
            let is_paused = this.update(cx, |pane, _| {
                pane.loop_ref(loop_id).map(|l| l.paused).unwrap_or(false)
            }).unwrap_or(false);
            if !is_paused {
                break;
            }
            let _ = this.update(cx, |pane, cx| {
                if let Some(l) = pane.loop_mut(loop_id) {
                    l.state = LoopState::Paused;
                    l.loop_phase = Default::default();
                }
                cx.notify();
            });
            Timer::after(Duration::from_millis(200)).await;
            let should_quit = this.update(cx, |pane, _| {
                pane.loop_ref(loop_id).map(|l| l.quit_requested).unwrap_or(true)
            }).unwrap_or(true);
            if should_quit {
                let _ = this.update(cx, |pane, cx| {
                    if let Some(l) = pane.loop_mut(loop_id) {
                        l.state = LoopState::Completed;
                        l.loop_phase = Default::default();
                    }
                    cx.notify();
                });
                return;
            }
        }

        // 6a. Ensure AGENT.md exists in plans dir
        let plans_dir_for_agent = effective_config.plans_dir.clone();
        if let Err(e) = smol::unblock(move || ensure_agent_md(&plans_dir_for_agent)).await {
            let _ = this.update(cx, |pane, cx| {
                pane.add_loop_output(loop_id, &format!("Warning: could not create AGENT.md: {}", e), true);
                cx.notify();
            });
        }

        // 6b. Build prompt
        let _ = this.update(cx, |pane, cx| {
            if let Some(l) = pane.loop_mut(loop_id) {
                l.loop_phase = super::types::LoopPhase::BuildingPrompt;
            }
            cx.notify();
        });
        let docs_dir = effective_config.docs_dir.clone();
        let plans_dir_for_prompt = effective_config.plans_dir.clone();
        let prompt = match smol::unblock(move || build_prompt(&docs_dir, &plans_dir_for_prompt)).await {
            Ok(p) => p,
            Err(e) => {
                let _ = this.update(cx, |pane, cx| {
                    pane.add_loop_output(loop_id, &format!("Failed to build prompt: {}", e), true);
                    if let Some(l) = pane.loop_mut(loop_id) {
                        l.state = LoopState::Completed;
                        l.loop_phase = Default::default();
                    }
                    cx.notify();
                });
                break;
            }
        };

        // 7. Update state to Running, increment iteration
        let _ = this.update(cx, |pane, cx| {
            if let Some(l) = pane.loop_mut(loop_id) {
                l.state = LoopState::Running;
                l.iteration += 1;
                l.iteration_start_time = Some(std::time::Instant::now());
                l.add_output(&format!("--- Iteration {} ---", l.iteration), false);
            }
            cx.notify();
        });

        // Write pre-run metadata to issue frontmatter
        let pre_run_issue_num = first_pending_number.clone().unwrap_or_default();
        if !pre_run_issue_num.is_empty() {
            let pre_docs_dir = effective_config.docs_dir.clone();
            let agent_name = effective_config.agent.clone();
            let model_name = effective_config.model.clone();
            let current_iteration = this
                .update(cx, |pane, _| pane.loop_ref(loop_id).map(|l| l.iteration).unwrap_or(0))
                .unwrap_or(0);
            let iteration_str = current_iteration.to_string();
            let started_at = iso_now();
            let _ = smol::unblock(move || {
                if let Some(path) = find_issue_file_path(&pre_docs_dir, &pre_run_issue_num) {
                    let _ = update_issue_frontmatter(&path, &[
                        ("startedAt", &started_at),
                        ("agent", &agent_name),
                        ("model", &model_name),
                        ("iteration", &iteration_str),
                    ]);
                }
            })
            .await;
        }

        // 8. Git snapshot before
        let pp = project_path.clone();
        let before_snapshot = smol::unblock(move || get_snapshot(&pp)).await;

        // 9. Spawn agent
        let _ = this.update(cx, |pane, cx| {
            if let Some(l) = pane.loop_mut(loop_id) {
                l.loop_phase = super::types::LoopPhase::SpawningAgent;
            }
            cx.notify();
        });
        let agent_result = {
            let cfg = effective_config.clone();
            let pp = project_path.clone();
            let p = prompt.clone();
            smol::unblock(move || spawn_agent(&cfg, &pp, &p)).await
        };

        let mut handle = match agent_result {
            Ok(h) => h,
            Err(e) => {
                let _ = this.update(cx, |pane, cx| {
                    pane.add_loop_output(loop_id, &format!("Failed to spawn agent: {}", e), true);
                    if let Some(l) = pane.loop_mut(loop_id) {
                        l.fail_count += 1;
                    }
                    cx.notify();
                });
                continue;
            }
        };

        // 10. Stream output lines
        let _ = this.update(cx, |pane, cx| {
            if let Some(l) = pane.loop_mut(loop_id) {
                l.loop_phase = super::types::LoopPhase::AgentRunning;
            }
            cx.notify();
        });
        let mut full_output = String::new();
        loop {
            // Check quit/skip
            let (should_quit, should_skip) = this
                .update(cx, |pane, _| {
                    pane.loop_ref(loop_id)
                        .map(|l| (l.quit_requested, l.skip_requested))
                        .unwrap_or((true, false))
                })
                .unwrap_or((true, false));

            if should_quit || should_skip {
                handle.kill();
                if should_skip {
                    let _ = this.update(cx, |pane, cx| {
                        if let Some(l) = pane.loop_mut(loop_id) {
                            l.skip_requested = false;
                            l.add_output("Skipped.", false);
                        }
                        cx.notify();
                    });
                }
                if should_quit {
                    let _ = this.update(cx, |pane, cx| {
                        if let Some(l) = pane.loop_mut(loop_id) {
                            l.state = LoopState::Completed;
                            l.loop_phase = Default::default();
                        }
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
                        pane.add_loop_output(loop_id, &line, false);
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
                    pane.add_loop_output(loop_id, &line, true);
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
        let _ = this.update(cx, |pane, cx| {
            if let Some(l) = pane.loop_mut(loop_id) {
                l.loop_phase = super::types::LoopPhase::WaitingForExit;
            }
            cx.notify();
        });
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
                pane.add_loop_output(loop_id, &line, false);
                cx.notify();
            });
        }

        // 12. Check done sentinel
        let _ = this.update(cx, |pane, cx| {
            if let Some(l) = pane.loop_mut(loop_id) {
                l.loop_phase = super::types::LoopPhase::CheckingResults;
            }
            cx.notify();
        });
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
            if let Some(l) = pane.loop_mut(loop_id) {
                l.diff_stat = diff_stat;
                if exit_code == 0 {
                    l.pass_count += 1;
                } else {
                    l.fail_count += 1;
                }
                l.add_output(
                    &format!("Agent exited with code {}", exit_code),
                    exit_code != 0,
                );
            }
            cx.notify();
        });

        // Write post-run metadata to issue frontmatter
        let post_run_issue_num = first_pending_number.clone().unwrap_or_default();
        if !post_run_issue_num.is_empty() {
            let post_docs_dir = effective_config.docs_dir.clone();
            let exit_code_str = exit_code.to_string();
            let duration_secs = this
                .update(cx, |pane, _| {
                    pane.loop_ref(loop_id)
                        .and_then(|l| l.iteration_start_time.map(|t| t.elapsed().as_secs()))
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            let duration_str = format!("{}s", duration_secs);
            let ended_at = iso_now();
            let _ = smol::unblock(move || {
                if let Some(path) = find_issue_file_path(&post_docs_dir, &post_run_issue_num) {
                    let _ = update_issue_frontmatter(&path, &[
                        ("endedAt", &ended_at),
                        ("exitCode", &exit_code_str),
                        ("duration", &duration_str),
                    ]);
                }
            })
            .await;
        }

        if is_done {
            let _ = this.update(cx, |pane, cx| {
                if let Some(l) = pane.loop_mut(loop_id) {
                    l.state = LoopState::Completed;
                    l.loop_phase = Default::default();
                    l.add_output("Agent signaled completion.", false);
                }
                cx.notify();
            });
            break;
        }

        // 15. Step mode: wait for user advancement
        let step_mode = this.update(cx, |pane, _| {
            pane.loop_ref(loop_id).map(|l| l.step_mode).unwrap_or(false)
        }).unwrap_or(false);
        if step_mode {
            let _ = this.update(cx, |pane, cx| {
                if let Some(l) = pane.loop_mut(loop_id) {
                    l.state = LoopState::WaitingForStep;
                    l.loop_phase = Default::default();
                }
                cx.notify();
            });
            loop {
                let (quit, skip, step_advance) = this
                    .update(cx, |pane, _| {
                        pane.loop_ref(loop_id)
                            .map(|l| (l.quit_requested, l.skip_requested, l.step_advance_requested))
                            .unwrap_or((true, false, false))
                    })
                    .unwrap_or((true, false, false));
                if quit || skip || step_advance {
                    let _ = this.update(cx, |pane, _| {
                        if let Some(l) = pane.loop_mut(loop_id) {
                            l.step_advance_requested = false;
                        }
                    });
                    if quit {
                        let _ = this.update(cx, |pane, cx| {
                            if let Some(l) = pane.loop_mut(loop_id) {
                                l.state = LoopState::Completed;
                                l.loop_phase = Default::default();
                            }
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
            let sleep_secs = effective_config.sleep_secs;
            let _ = this.update(cx, |pane, cx| {
                if let Some(l) = pane.loop_mut(loop_id) {
                    l.loop_phase = super::types::LoopPhase::Sleeping(sleep_secs);
                    l.add_output(&format!("Sleeping {}s...", sleep_secs), false);
                }
                cx.notify();
            });
            Timer::after(Duration::from_secs(sleep_secs)).await;
        }
    }

    // Final cleanup
    let _ = this.update(cx, |pane, cx| {
        if let Some(l) = pane.loop_mut(loop_id) {
            if l.state != LoopState::Completed {
                l.state = LoopState::Completed;
            }
            l.loop_phase = Default::default();
        }
        cx.notify();
    });
}
