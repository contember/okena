# Issue 09: KruhPane async loop runner

**Priority:** high
**Files:** `src/views/layout/kruh_pane/loop_runner.rs` (new)

## Description

Create the main iteration loop that drives the kruh agent. This runs as a `cx.spawn()` async task, communicating with the KruhPane entity via `WeakEntity` — the same pattern used by Okena's PTY reader loops.

## New file: `src/views/layout/kruh_pane/loop_runner.rs`

### `start_loop()` function

Called from `KruhPane::start_loop()`. Spawns the async loop task:

```rust
use gpui::*;
use smol::Timer;
use std::time::{Duration, Instant};
use super::KruhPane;
use super::types::{KruhState, OutputLine, StatusProgress};
use super::agent::{spawn_agent, AgentHandle};
use super::git::{get_snapshot, get_diff_stat};
use super::status_parser::{parse_status, build_prompt, check_done_sentinel};

pub fn start_loop(this: WeakEntity<KruhPane>, cx: &mut Context<KruhPane>) -> gpui::Task<()> {
    cx.spawn(async move |mut cx| {
        run_loop(this, &mut cx).await;
    })
}
```

### `run_loop()` — the main async function

```rust
async fn run_loop(this: WeakEntity<KruhPane>, cx: &mut AsyncWindowContext) {
    loop {
        // 1. Check quit/skip
        let should_quit = this.update(cx, |pane, _cx| pane.quit_requested).unwrap_or(true);
        if should_quit {
            break;
        }

        // 2. Get config and project path
        let (config, project_path, iteration, max_iterations) = match this.update(cx, |pane, _cx| {
            (pane.config.clone(), pane.project_path.clone(), pane.iteration, pane.config.max_iterations)
        }) {
            Ok(v) => v,
            Err(_) => break, // Entity dropped
        };

        // 3. Check max iterations
        if iteration > max_iterations {
            let _ = this.update(cx, |pane, cx| {
                pane.state = KruhState::Completed;
                pane.add_output("Max iterations reached.", false);
                cx.notify();
            });
            break;
        }

        // 4. Parse STATUS.md (blocking I/O on background thread)
        let docs_dir = config.docs_dir.clone();
        let progress = smol::unblock(move || parse_status(&docs_dir)).await;

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

        // 5. Check pause
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
                return;
            }
        }

        // 6. Build prompt
        let docs_dir = config.docs_dir.clone();
        let prompt = match smol::unblock(move || build_prompt(&docs_dir)).await {
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

        // 7. Update state to Running
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

        // Store handle reference for kill
        let _ = this.update(cx, |pane, _cx| {
            // We can't store the handle directly since we need it here,
            // but we track that an agent is running
        });

        // 10. Stream output lines
        let mut full_output = String::new();
        loop {
            // Check quit/skip
            let (should_quit, should_skip) = this.update(cx, |pane, _| {
                (pane.quit_requested, pane.skip_requested)
            }).unwrap_or((true, false));

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

            // Try to receive output lines (non-blocking)
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
                Err(async_channel::TryRecvError::Closed) => {
                    // Reader thread finished — agent likely exited
                    break;
                }
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
                Ok(Some(_code)) => break,
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
                Ok(None) => Timer::after(Duration::from_millis(100)).await,
                Err(_) => break -1,
            }
        };

        // Drain remaining output
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
        let pp = project_path.clone();
        let before = before_snapshot.clone();
        let diff_stat = smol::unblock(move || {
            let after = get_snapshot(&pp);
            match (before, after) {
                (Some(b), Some(a)) if b != a => get_diff_stat(&pp, &b, &a),
                _ => None,
            }
        }).await;

        // Note: get_diff_stat needs project_path but we moved pp above.
        // Restructure to avoid the move issue — clone project_path again.

        // 14. Update state
        let _ = this.update(cx, |pane, cx| {
            pane.diff_stat = diff_stat;
            if exit_code == 0 {
                pane.pass_count += 1;
            } else {
                pane.fail_count += 1;
            }
            pane.add_output(&format!("Agent exited with code {}", exit_code), exit_code != 0);
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

        // 15. Step mode: wait for Enter
        let step_mode = this.update(cx, |pane, _| pane.step_mode).unwrap_or(false);
        if step_mode {
            let _ = this.update(cx, |pane, cx| {
                pane.state = KruhState::WaitingForStep;
                cx.notify();
            });
            loop {
                let (quit, skip, step_advance) = this.update(cx, |pane, _| {
                    (pane.quit_requested, pane.skip_requested, pane.step_advance_requested)
                }).unwrap_or((true, false, false));
                if quit || skip || step_advance {
                    let _ = this.update(cx, |pane, _| {
                        pane.step_advance_requested = false;
                    });
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
```

Note: The `this.update(cx, ...)` pattern matches how Okena's terminal background tasks communicate with their entities. Check the exact API — it might be `this.update(&mut cx, ...)` or similar depending on the GPUI version.

The `step_advance_requested` field needs to be added to KruhPane (set to `true` when Enter is pressed in WaitingForStep state).

### Important: Variable ownership

The code above has some ownership issues with `project_path` being moved into closures. The actual implementation should clone `project_path` before each `smol::unblock` call. The pseudocode above shows the logic flow — fix the ownership on implementation.

## Acceptance Criteria

- Loop iterates: parse → prompt → spawn → stream → diff → sleep/step
- Output lines stream in real-time to the entity
- Pause/skip/quit controls work during the loop
- Step mode waits for user advancement
- Done sentinel detected and stops the loop
- Max iterations limit enforced
- Agent process killed on skip/quit
- Loop exits cleanly and sets Completed state
- `cargo build` succeeds
