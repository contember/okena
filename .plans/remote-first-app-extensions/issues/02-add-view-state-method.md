# Issue 02: Add view_state() to KruhPane

**Priority:** high
**Files:** `src/views/layout/kruh_pane/mod.rs`

## Description

Add a `pub fn view_state(&self, cx: &Context<Self>) -> KruhViewState` method that maps KruhPane's internal GPUI state into the pure-data `KruhViewState` from issue 01.

## Implementation

### `src/views/layout/kruh_pane/mod.rs`

Add a method to `impl KruhPane`:

```rust
pub fn view_state(&self, _cx: &Context<Self>) -> KruhViewState {
    let screen = match &self.state {
        KruhState::Scanning => KruhScreen::Scanning,
        KruhState::PlanPicker => {
            KruhScreen::PlanPicker {
                plans: self.plans.iter().map(|p| PlanViewInfo {
                    name: p.name.clone(),
                    path: p.path.display().to_string(),
                    issue_count: p.issue_count,
                    completed_count: p.completed_count,
                }).collect(),
                selected_index: self.selected_plan_index,
            }
        }
        KruhState::TaskBrowser => {
            // Map from current task browser state
            KruhScreen::TaskBrowser {
                plan_name: self.current_plan_name(),
                issues: self.current_issues_view(),
            }
        }
        KruhState::Editing => {
            KruhScreen::Editing {
                file_path: self.editor_file_path(),
                content: self.editor_content(),
                is_new: self.editor_is_new,
            }
        }
        KruhState::Settings => {
            KruhScreen::Settings {
                model: self.config.model.clone(),
                max_iterations: self.config.max_iterations,
                auto_start: self.config.auto_start,
            }
        }
        KruhState::LoopOverview => {
            KruhScreen::LoopOverview {
                loops: self.active_loops.iter().map(|l| LoopViewInfo {
                    loop_id: l.id,
                    plan_name: l.plan_name.clone(),
                    phase: format!("{}", l.phase), // Use Display impl
                    state: l.state_str(),
                    current_issue: l.current_issue.clone(),
                    progress: ProgressViewInfo {
                        completed: l.progress.completed,
                        total: l.progress.total,
                    },
                    // Cap output at last 200 lines
                    output_lines: l.output.iter().rev().take(200).rev().map(|o| OutputLineView {
                        text: o.text.clone(),
                        is_error: o.is_error,
                    }).collect(),
                }).collect(),
                focused_index: self.focused_loop_index,
            }
        }
    };

    KruhViewState {
        app_id: self.app_id.clone(),
        screen,
    }
}
```

The exact field names and accessor methods will depend on KruhPane's actual internal structure — adapt the mapping to match real field names. The key constraint: NO GPUI handles (`ScrollHandle`, `FocusHandle`, `SimpleInputState`) appear in the output.

## Acceptance Criteria

- `view_state()` covers all 6 `KruhState` variants
- Output lines capped at 200 per loop
- No GPUI types in return value
- No `Instant` in return value
- Method is `pub` so the registry can call it
- `cargo build` succeeds
