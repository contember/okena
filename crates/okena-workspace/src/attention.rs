use crate::{context::WorkspaceCx, state::Workspace};
use okena_core::{agent_status::AgentStatus, attention::AttentionSource};

impl Workspace {
    /// Call only after the PTY owner validated the event's attachment/generation.
    pub fn record_attention(
        &mut self,
        source: AttentionSource,
        status: Option<&AgentStatus>,
        timestamp: u64,
        cx: &mut impl WorkspaceCx,
    ) {
        if !self
            .data
            .projects
            .iter()
            .any(|p| p.id == source.project_id && p.connection_id.is_none())
        {
            return;
        }
        let mut changed = false;
        if let Some(conversation) = source.conversation.as_ref() {
            changed |= crate::missions::promote_conversation(
                &mut self.data,
                &source.project_id,
                &source.terminal_id,
                conversation.clone(),
            );
        }
        changed |=
            self.data
                .attention
                .record(source, status, timestamp, uuid::Uuid::new_v4().to_string());
        if changed {
            self.notify_data(cx);
        }
    }

    pub fn invalidate_attention_terminal(&mut self, terminal_id: &str, cx: &mut impl WorkspaceCx) {
        if self.data.attention.invalidate_terminal(terminal_id) {
            self.notify_data(cx);
        }
    }

    pub fn record_attention_loss(
        &mut self,
        terminal_id: &str,
        count: u64,
        cx: &mut impl WorkspaceCx,
    ) {
        if count > 0 {
            self.data.attention.record_loss(terminal_id, count);
            self.notify_data(cx);
        }
    }

    pub fn acknowledge_attention(
        &mut self,
        episode_id: &str,
        revision: u64,
        dismiss: bool,
        cx: &mut impl WorkspaceCx,
    ) -> bool {
        let changed = self
            .data
            .attention
            .acknowledge(episode_id, revision, dismiss);
        if changed {
            self.notify_data(cx);
        }
        changed
    }
}
