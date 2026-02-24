# Issue 08: KruhPane GPUI render implementation

**Priority:** high
**Files:** `src/views/layout/kruh_pane/render.rs` (new)

## Description

Create the GPUI render implementation for KruhPane. This defines the visual layout of the kruh app pane with all its UI regions.

## New file: `src/views/layout/kruh_pane/render.rs`

The render function produces different layouts based on `KruhState`:
- **Idle**: Config panel with editable fields + "Start" button
- **Running/Paused/WaitingForStep**: Header + progress + output + controls
- **Completed**: Header + progress + output + summary

### Overall structure

```rust
use gpui::*;
use crate::theme::theme;
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use crate::views::components::ui_helpers::*;
use super::KruhPane;
use super::types::KruhState;

impl KruhPane {
    pub fn render_view(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.foreground)
            .child(self.render_header(window, cx))
            .child(match &self.state {
                KruhState::Idle => self.render_config_panel(window, cx).into_any_element(),
                _ => self.render_running_view(window, cx).into_any_element(),
            })
    }
}
```

### 1. Header bar

```rust
fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);
    div()
        .flex()
        .items_center()
        .px_2()
        .py_1()
        .gap_2()
        .border_b_1()
        .border_color(t.border)
        .child(
            svg()
                .path("icons/kruh.svg")
                .size_4()
                .text_color(t.foreground)
        )
        .child(
            div().text_sm().font_weight(FontWeight::BOLD).child("Kruh")
        )
        .child(
            div().text_xs().text_color(t.muted_foreground).child(
                format!("{} / {}", self.config.agent, self.config.model)
            )
        )
        .when(!self.config.docs_dir.is_empty(), |el| {
            el.child(
                div().text_xs().text_color(t.muted_foreground).child(
                    format!("docs: {}", self.config.docs_dir)
                )
            )
        })
}
```

### 2. Progress bar

```rust
fn render_progress_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);
    let total = self.progress.total.max(1) as f32;
    let ratio = self.progress.done as f32 / total;
    let pct = (ratio * 100.0) as usize;

    let bar_color = if ratio < 0.25 {
        t.error  // red
    } else if ratio < 0.75 {
        t.warning  // yellow
    } else {
        t.success  // green
    };

    div()
        .px_2()
        .py_1()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div().text_xs().child(format!("{}/{}", self.progress.done, self.progress.total))
        )
        .child(
            div()
                .flex_1()
                .h(px(6.0))
                .rounded_sm()
                .bg(t.muted)
                .child(
                    div()
                        .h_full()
                        .rounded_sm()
                        .bg(bar_color)
                        .w(relative(ratio))
                )
        )
        .child(
            div().text_xs().child(format!("{}%", pct))
        )
}
```

### 3. Iteration banner

```rust
fn render_iteration_banner(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);
    let elapsed = self.start_time.map(|t| t.elapsed()).unwrap_or_default();
    let mins = elapsed.as_secs() / 60;
    let secs = elapsed.as_secs() % 60;

    div()
        .px_2()
        .py_1()
        .flex()
        .justify_between()
        .bg(t.muted)
        .child(
            div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                format!("Iteration {}/{}", self.iteration, self.config.max_iterations)
            )
        )
        .child(
            div().text_sm().text_color(t.muted_foreground).child(
                format!("{:02}:{:02}", mins, secs)
            )
        )
}
```

### 4. Output display

Scrollable div with styled output lines. Auto-scrolls to bottom. Basic ANSI stripping.

```rust
fn render_output(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);

    div()
        .flex_1()
        .overflow_y_scroll()
        .track_scroll(&self.output_scroll)
        .px_2()
        .py_1()
        .children(
            self.output_lines.iter().map(|line| {
                let text_color = if line.is_error { t.error } else { t.foreground };
                div()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(text_color)
                    .child(strip_ansi(&line.text))
            })
        )
}
```

### Helper: `strip_ansi()`

```rust
fn strip_ansi(text: &str) -> String {
    // Remove ANSI escape sequences: \x1b[...m and similar
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we find the terminating character (letter)
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}
```

### 5. Diff display

```rust
fn render_diff(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);

    div()
        .px_2()
        .py_1()
        .border_t_1()
        .border_color(t.border)
        .when_some(self.diff_stat.as_ref(), |el, stat| {
            el.children(stat.lines().map(|line| {
                let color = if line.contains('+') && !line.contains('-') {
                    t.success
                } else if line.contains('-') && !line.contains('+') {
                    t.error
                } else {
                    t.foreground
                };
                div().text_xs().font_family("monospace").text_color(color).child(line.to_string())
            }))
        })
}
```

### 6. Control bar

```rust
fn render_controls(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);
    let state_label = match self.state {
        KruhState::Running => "Running",
        KruhState::Paused => "Paused",
        KruhState::WaitingForStep => "Waiting (press Enter)",
        KruhState::Completed => "Completed",
        KruhState::Idle => "Idle",
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .px_2()
        .py_1()
        .border_t_1()
        .border_color(t.border)
        .bg(t.muted)
        .child(
            div().flex().gap_3().children([
                keyboard_hint("P", "Pause"),
                keyboard_hint("S", "Skip"),
                keyboard_hint("Q", "Quit"),
                keyboard_hint("T", "Step"),
            ])
        )
        .child(
            div().flex().gap_2()
                .child(div().text_xs().text_color(t.muted_foreground).child(
                    format!("Pass: {} Fail: {}", self.pass_count, self.fail_count)
                ))
                .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child(state_label))
        )
}
```

### 7. Config panel (Idle state)

Uses `SimpleInput` components for editable fields:

```rust
fn render_config_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let t = theme(cx);

    div()
        .flex_1()
        .flex()
        .flex_col()
        .p_4()
        .gap_3()
        // Docs dir input
        .child(labeled_input("Docs Directory", SimpleInput::new(&self.docs_dir_input).text_size(px(12.0))))
        // Agent selector (dropdown or text input)
        .child(labeled_input("Agent", SimpleInput::new(&self.agent_input).text_size(px(12.0))))
        // Model input
        .child(labeled_input("Model", SimpleInput::new(&self.model_input).text_size(px(12.0))))
        // Max iterations input
        .child(labeled_input("Max Iterations", SimpleInput::new(&self.max_iterations_input).text_size(px(12.0))))
        // Dangerous toggle
        .child(labeled_input("Dangerous Mode", SimpleInput::new(&self.dangerous_input).text_size(px(12.0))))
        // Start button
        .child(
            div().flex().justify_center().pt_2().child(
                button_primary("Start", cx.listener(|this, _, window, cx| {
                    this.start_loop(window, cx);
                }))
            )
        )
}
```

Note: The KruhPane struct needs `SimpleInputState` entities for each config field. These should be initialized in `KruhPane::new()`:
- `docs_dir_input: Entity<SimpleInputState>`
- `agent_input: Entity<SimpleInputState>`
- `model_input: Entity<SimpleInputState>`
- `max_iterations_input: Entity<SimpleInputState>`
- `dangerous_input: Entity<SimpleInputState>`

### Running view (combines progress + iteration + output + diff + controls)

```rust
fn render_running_view(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .child(self.render_progress_bar(window, cx))
        .child(self.render_iteration_banner(window, cx))
        .child(self.render_output(window, cx))
        .when(self.diff_stat.is_some(), |el| {
            el.child(self.render_diff(window, &*cx))
        })
        .child(self.render_controls(window, cx))
}
```

### Keyboard handling

Register key bindings in the render method or via `on_key_down`:
- `p` → toggle `self.paused`
- `s` → set `self.skip_requested = true`
- `q` → set `self.quit_requested = true`
- `t` → toggle `self.step_mode`
- `Enter` → advance step (when in `WaitingForStep`)

These should only be active when the pane is focused and in a running state.

## Acceptance Criteria

- All 7 UI regions render correctly
- Config panel shows editable fields with SimpleInput
- Progress bar reflects actual progress with color coding
- Output display auto-scrolls and strips ANSI codes
- Keyboard shortcuts dispatch correctly
- Theme colors are used consistently
- `cargo build` succeeds
