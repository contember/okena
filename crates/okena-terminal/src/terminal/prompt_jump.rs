use alacritty_terminal::grid::Scroll;
use std::sync::atomic::Ordering;

use super::Terminal;
use super::types::{JumpDirection, PromptMark, PromptMarkKind};

impl Terminal {
    /// Snapshot of the OSC 133 shell-integration marks currently tracked
    /// for this terminal, oldest first. Returns an empty Vec when the
    /// running shell has no OSC 133 support enabled.
    pub fn prompt_marks(&self) -> Vec<PromptMark> {
        self.prompt_tracker.lock().snapshot()
    }

    /// Scroll the viewport so the next older `OSC 133 ; A` prompt lands at
    /// visual row 0. The first call after any shell output lands on the
    /// most-recent prompt (even if it's already visible); each subsequent
    /// call walks one prompt further into history until there are none
    /// left, at which point `false` is returned.
    pub fn jump_to_prompt_above(&self) -> bool {
        self.jump_to_prompt(JumpDirection::Above)
    }

    /// Reverse of [`Terminal::jump_to_prompt_above`]: walks one prompt forward toward
    /// the live bottom. Returns `false` when the walker is already sitting
    /// on the newest prompt or hasn't started walking yet.
    pub fn jump_to_prompt_below(&self) -> bool {
        self.jump_to_prompt(JumpDirection::Below)
    }

    fn jump_to_prompt(&self, direction: JumpDirection) -> bool {
        let marks = self.prompt_tracker.lock().snapshot();
        // Only `PromptStart` is a reliable "prompt begins here" marker.
        let prompts: Vec<&PromptMark> = marks
            .iter()
            .filter(|m| m.kind == PromptMarkKind::PromptStart)
            .collect();
        if prompts.is_empty() {
            return false;
        }

        // `prompt_jump_index` is a reverse index into `prompts`: 0 = newest,
        // 1 = one older, etc. `None` means "walker is not engaged; an
        // Above jump should land on the newest prompt". Storing a reverse
        // index keeps the walk scroll-invariant — scrolling rebases line
        // values on every mark, but the relative order and count don't
        // change.
        let new_index: usize = {
            let mut state = self.prompt_jump_index.lock();
            let next = match (direction, *state) {
                (JumpDirection::Above, None) => 0,
                (JumpDirection::Above, Some(n)) => {
                    if n + 1 >= prompts.len() {
                        return false;
                    }
                    n + 1
                }
                (JumpDirection::Below, Some(n)) if n > 0 => n - 1,
                (JumpDirection::Below, _) => return false,
            };
            *state = Some(next);
            next
        };

        let target = prompts[prompts.len() - 1 - new_index];
        self.scroll_to_prompt_mark(target);
        true
    }

    /// Scroll the viewport so the next older prompt whose command finished
    /// with a non-zero exit code lands at visual row 0. Mirrors
    /// [`Terminal::jump_to_prompt_above`] but only visits failures.
    pub fn jump_to_prev_failed_command(&self) -> bool {
        self.jump_to_failed(JumpDirection::Above)
    }

    /// Reverse of [`Terminal::jump_to_prev_failed_command`]: walks one
    /// failure forward toward the live bottom. Returns `false` when the
    /// walker is already sitting on the newest failure or hasn't started
    /// walking yet.
    pub fn jump_to_next_failed_command(&self) -> bool {
        self.jump_to_failed(JumpDirection::Below)
    }

    fn jump_to_failed(&self, direction: JumpDirection) -> bool {
        let marks = self.prompt_tracker.lock().snapshot();
        // Build the ordered list of prompts that produced a failure: walk
        // marks chronologically tracking the most recent `PromptStart`, and
        // whenever a `CommandFinished` reports a non-zero exit code, record
        // the prompt that started that command. Marks with an unknown exit
        // code (`None`) or a zero exit are not failures.
        let mut last_prompt: Option<&PromptMark> = None;
        let mut prompts: Vec<&PromptMark> = Vec::new();
        for mark in &marks {
            match mark.kind {
                PromptMarkKind::PromptStart => last_prompt = Some(mark),
                PromptMarkKind::CommandFinished {
                    exit_code: Some(code),
                } if code != 0 => {
                    if let Some(prompt) = last_prompt {
                        prompts.push(prompt);
                    }
                }
                _ => {}
            }
        }
        if prompts.is_empty() {
            return false;
        }

        // `failed_jump_index` is a reverse index into `prompts`: 0 = newest
        // failure, 1 = one older, etc. `None` means "walker is not engaged;
        // an Above jump should land on the newest failure". Storing a
        // reverse index keeps the walk scroll-invariant — scrolling rebases
        // line values on every mark, but the relative order and count don't
        // change.
        let new_index: usize = {
            let mut state = self.failed_jump_index.lock();
            let next = match (direction, *state) {
                (JumpDirection::Above, None) => 0,
                (JumpDirection::Above, Some(n)) => {
                    if n + 1 >= prompts.len() {
                        return false;
                    }
                    n + 1
                }
                (JumpDirection::Below, Some(n)) if n > 0 => n - 1,
                (JumpDirection::Below, _) => return false,
            };
            *state = Some(next);
            next
        };

        let target = prompts[prompts.len() - 1 - new_index];
        self.scroll_to_prompt_mark(target);
        true
    }

    /// Scroll the viewport so `target` lands at visual row 0, bypassing
    /// `self.scroll` so the jump walker state isn't cleared — `self.scroll`
    /// is reserved for externally-driven scrolling which resets the walker.
    fn scroll_to_prompt_mark(&self, target: &PromptMark) {
        let target_offset = (-target.line).max(0);

        let mut term = self.term.lock();
        let current = term.grid().display_offset() as i32;
        let delta = target_offset - current;
        if delta != 0 {
            term.scroll_display(Scroll::Delta(delta));
            drop(term);
            *self.scroll_offset.lock() += delta;
            self.content_generation.fetch_add(1, Ordering::Relaxed);
        }
    }
}
