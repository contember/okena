# Issue 03: Add handle_action() to KruhPane

**Priority:** high
**Files:** `src/views/layout/kruh_pane/mod.rs`

## Description

Add a `pub fn handle_action(&mut self, action: KruhAction, cx: &mut Context<Self>)` method that dispatches each `KruhAction` variant to the corresponding existing mutation method on KruhPane.

## Implementation

### `src/views/layout/kruh_pane/mod.rs`

```rust
pub fn handle_action(&mut self, action: KruhAction, cx: &mut Context<Self>) {
    match action {
        KruhAction::StartScan => self.start_scan(cx),
        KruhAction::SelectPlan { index } => self.select_plan(index, cx),
        KruhAction::OpenPlan { name } => self.open_plan(&name, cx),
        KruhAction::BackToPlans => self.back_to_plans(cx),
        KruhAction::StartLoop { plan_name } => self.start_loop_from_plan(&plan_name, cx),
        KruhAction::StartAllLoops => self.start_all_loops(cx),
        KruhAction::PauseLoop { loop_id } => self.pause_loop(loop_id, cx),
        KruhAction::ResumeLoop { loop_id } => self.resume_loop(loop_id, cx),
        KruhAction::StopLoop { loop_id } => self.stop_loop(loop_id, cx),
        KruhAction::CloseLoops => self.close_loops(cx),
        KruhAction::FocusLoop { index } => self.focus_loop(index, cx),
        KruhAction::OpenEditor { file_path } => self.open_editor(&file_path, cx),
        KruhAction::SaveEditor { content } => self.save_editor(&content, cx),
        KruhAction::CloseEditor => self.close_editor(cx),
        KruhAction::OpenSettings => self.open_settings(cx),
        KruhAction::UpdateSettings { model, max_iterations, auto_start } => {
            self.update_settings(model, max_iterations, auto_start, cx);
        }
        KruhAction::CloseSettings => self.close_settings(cx),
        KruhAction::BrowseTasks { plan_name } => self.browse_tasks(&plan_name, cx),
    }
}
```

The method names must match KruhPane's actual API. If a method doesn't exist yet (e.g., `pause_loop`), add a minimal stub. Some actions may need adaptation:

- **Window-dependent operations** (focus, scroll): Skip when called from remote context. Since we can't easily detect "remote vs local" at this level, these methods should be safe to call regardless — GPUI will no-op if there's no window context.
- **Editor content**: `SaveEditor` receives full content from remote and applies it.

## Acceptance Criteria

- Every `KruhAction` variant has a match arm
- Each arm delegates to an existing method (or a new stub if needed)
- No panics when called from any state
- `cargo build` succeeds
