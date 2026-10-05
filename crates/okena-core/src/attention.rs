//! Authoritative current-work attention. Only the PTY owner feeds transitions.

use crate::agent_session::{AgentSession, is_uuid_like, is_valid_agent_id};
use crate::agent_status::{AgentLifecycle, AgentStatus};
use serde::{Deserialize, Serialize};

/// Retired anonymous attachments cannot be reconciled to a durable conversation.
pub const MAX_UNAVAILABLE_ANONYMOUS_SOURCES: usize = 128;

/// Public conversation identity, deliberately without a transcript path.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId {
    pub agent: String,
    pub session_id: String,
}

impl ConversationId {
    pub fn is_valid(&self) -> bool {
        is_valid_agent_id(&self.agent) && is_uuid_like(&self.session_id)
    }
}

impl From<&AgentSession> for ConversationId {
    fn from(session: &AgentSession) -> Self {
        Self {
            agent: session.agent.clone(),
            session_id: session.session_id.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionSource {
    pub project_id: String,
    pub terminal_id: String,
    /// Owner-generated attachment ID, unique across daemon restarts.
    pub attachment_id: String,
    pub generation: u64,
    pub conversation: Option<ConversationId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    InputNeeded,
    Completion,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionEpisode {
    pub id: String,
    pub revision: u64,
    pub source: AttentionSource,
    pub kind: AttentionKind,
    pub summary: String,
    /// Unix seconds, assigned by the daemon.
    pub created_at: u64,
    pub updated_at: u64,
    /// The source's live status has not been cleared or invalidated.
    pub available: bool,
    /// The original terminal attachment can still be revealed after status clears.
    #[serde(default)]
    pub terminal_available: bool,
    pub read: bool,
}

impl AttentionEpisode {
    pub fn can_open_terminal(&self) -> bool {
        // Older daemons only confirm terminal availability through live status.
        self.terminal_available || self.available
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionObservation {
    pub source: AttentionSource,
    pub lifecycle: Option<AgentLifecycle>,
    pub input: Option<AttentionEpisode>,
    pub completion: Option<AttentionEpisode>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionState {
    #[serde(default)]
    pub observations: Vec<AttentionObservation>,
    /// Capture losses and anonymous retention evictions; this is not a full history.
    #[serde(default)]
    pub lost_transitions: u64,
}

impl AttentionState {
    pub fn validate_persisted(&mut self) {
        let mut episodes = std::collections::BTreeMap::<String, AttentionEpisode>::new();
        let mut conflicting_ids = std::collections::HashSet::new();
        for observation in std::mem::take(&mut self.observations) {
            if !valid_source(&observation.source) {
                continue;
            }
            for mut episode in [observation.input, observation.completion]
                .into_iter()
                .flatten()
            {
                let related = match episode.kind {
                    AttentionKind::InputNeeded => {
                        same_attachment(&episode.source, &observation.source)
                    }
                    AttentionKind::Completion => {
                        same_completion_source(&episode.source, &observation.source)
                    }
                };
                if !related
                    || !valid_source(&episode.source)
                    || !is_uuid_like(&episode.id)
                    || episode.revision == 0
                {
                    continue;
                }
                crate::agent_status::truncate_to_bytes(
                    &mut episode.summary,
                    crate::agent_status::MAX_CUSTOM_LEN,
                );
                if conflicting_ids.contains(&episode.id) {
                    continue;
                }
                if episodes.get(&episode.id).is_some_and(|old| {
                    old.kind != episode.kind || !same_attachment(&old.source, &episode.source)
                }) {
                    episodes.remove(&episode.id);
                    conflicting_ids.insert(episode.id);
                    continue;
                }
                let replace = episodes.get(&episode.id).is_none_or(|old| {
                    (
                        episode.revision,
                        episode.read,
                        episode.updated_at,
                        episode.created_at,
                    ) > (old.revision, old.read, old.updated_at, old.created_at)
                });
                if replace {
                    episodes.insert(episode.id.clone(), episode);
                }
            }
        }
        let mut episodes: Vec<_> = episodes.into_values().collect();
        episodes.sort_by(|a, b| {
            (b.created_at, b.updated_at, &b.id).cmp(&(a.created_at, a.updated_at, &a.id))
        });
        for episode in episodes {
            if episode.kind == AttentionKind::Completion
                && self.observations.iter().any(|o| {
                    o.completion
                        .as_ref()
                        .is_some_and(|e| same_completion_source(&e.source, &episode.source))
                })
            {
                continue;
            }
            let index = self
                .observations
                .iter()
                .position(|o| same_attachment(&o.source, &episode.source));
            let index = index.unwrap_or_else(|| {
                self.observations.push(AttentionObservation {
                    source: episode.source.clone(),
                    lifecycle: None,
                    input: None,
                    completion: None,
                });
                self.observations.len() - 1
            });
            let slot = match episode.kind {
                AttentionKind::InputNeeded => &mut self.observations[index].input,
                AttentionKind::Completion => &mut self.observations[index].completion,
            };
            if slot.is_none() {
                *slot = Some(episode);
            }
        }
        self.observations.reverse();
        self.restart();
    }
    /// The owner validates generation before calling. `episode_id` must be a fresh UUID.
    /// Known conversations supersede completions across attachments; input remains attachment-local.
    pub fn record(
        &mut self,
        source: AttentionSource,
        status: Option<&AgentStatus>,
        now: u64,
        episode_id: String,
    ) -> bool {
        if !is_uuid_like(&episode_id) || !valid_source(&source) {
            return false;
        }
        if self.observations.iter().any(|o| {
            o.source.terminal_id == source.terminal_id
                && o.source.attachment_id == source.attachment_id
                && o.source.generation > source.generation
        }) {
            return false;
        }
        let before = self.clone();
        // A new conversation or attachment must never inherit old read state.
        for observation in &mut self.observations {
            if observation.source.terminal_id == source.terminal_id
                && !same_attachment(&observation.source, &source)
            {
                make_unavailable(observation);
            }
        }
        let index = self
            .observations
            .iter()
            .position(|o| same_attachment(&o.source, &source));
        let observation = match index {
            Some(index) => &mut self.observations[index],
            None => {
                self.observations.push(AttentionObservation {
                    source: source.clone(),
                    lifecycle: None,
                    input: None,
                    completion: None,
                });
                let index = self.observations.len() - 1;
                &mut self.observations[index]
            }
        };
        if !same_attachment(&observation.source, &source) {
            make_unavailable(observation);
            observation.source = source.clone();
        } else if observation.source.project_id != source.project_id {
            observation.source.project_id = source.project_id.clone();
            for episode in [&mut observation.input, &mut observation.completion]
                .into_iter()
                .flatten()
            {
                if same_attachment(&episode.source, &source) {
                    episode.source.project_id = source.project_id.clone();
                }
            }
        }
        let Some(status) = status else {
            observation.lifecycle = None;
            for episode in [&mut observation.input, &mut observation.completion]
                .into_iter()
                .flatten()
            {
                episode.terminal_available = episode.can_open_terminal();
                episode.available = false;
            }
            self.prune_retired();
            return *self != before;
        };
        let mut summary = status
            .custom
            .as_deref()
            .unwrap_or(status.lifecycle.label())
            .to_owned();
        crate::agent_status::truncate_to_bytes(&mut summary, crate::agent_status::MAX_CUSTOM_LEN);
        let kind = match status.lifecycle {
            AgentLifecycle::Blocked => Some(AttentionKind::InputNeeded),
            AgentLifecycle::Done => Some(AttentionKind::Completion),
            _ => None,
        };
        if status.lifecycle != AgentLifecycle::Blocked {
            observation.input = None;
        }
        if let Some(kind) = kind {
            let slot = match kind {
                AttentionKind::InputNeeded => &mut observation.input,
                AttentionKind::Completion => &mut observation.completion,
            };
            if observation.lifecycle == Some(status.lifecycle) {
                if let Some(episode) = slot
                    && (episode.summary != summary || !episode.available)
                {
                    if let Some(revision) = episode.revision.checked_add(1) {
                        episode.revision = revision;
                    } else {
                        episode.id = episode_id;
                        episode.revision = 1;
                    }
                    episode.summary = summary;
                    episode.updated_at = now;
                    episode.available = true;
                    episode.terminal_available = true;
                    episode.read = false;
                }
            } else {
                *slot = Some(AttentionEpisode {
                    id: episode_id,
                    revision: 1,
                    source: source.clone(),
                    kind,
                    summary,
                    created_at: now,
                    updated_at: now,
                    available: true,
                    terminal_available: true,
                    read: false,
                });
            }
        }
        observation.lifecycle = Some(status.lifecycle);
        let completion_id = if status.lifecycle == AgentLifecycle::Done {
            observation.completion.as_ref().map(|e| e.id.clone())
        } else {
            None
        };
        if let Some(id) = completion_id {
            for other in &mut self.observations {
                if other
                    .completion
                    .as_ref()
                    .is_some_and(|e| e.id != id && same_completion_source(&e.source, &source))
                {
                    other.completion = None;
                }
            }
        }
        self.prune_retired();
        *self != before
    }

    fn prune_retired(&mut self) {
        self.observations.retain(|o| {
            o.lifecycle.is_some()
                || [&o.input, &o.completion]
                    .into_iter()
                    .flatten()
                    .any(|e| !e.read)
        });
        let mut remaining = MAX_UNAVAILABLE_ANONYMOUS_SOURCES;
        for index in (0..self.observations.len()).rev() {
            let o = &self.observations[index];
            if o.source.conversation.is_none() && o.lifecycle.is_none() {
                if remaining == 0 {
                    self.observations.remove(index);
                    self.lost_transitions = self.lost_transitions.saturating_add(1);
                } else {
                    remaining -= 1;
                }
            }
        }
    }

    pub fn episodes(&self) -> Vec<AttentionEpisode> {
        let mut episodes: Vec<_> = self
            .observations
            .iter()
            .flat_map(|o| [&o.input, &o.completion])
            .flatten()
            .filter(|e| !e.read)
            .cloned()
            .collect();
        episodes.sort_by(|a, b| {
            (a.kind == AttentionKind::Completion, a.created_at, &a.id).cmp(&(
                b.kind == AttentionKind::Completion,
                b.created_at,
                &b.id,
            ))
        });
        episodes
    }

    /// Reading a live question does not answer it. Unavailable questions require dismissal.
    pub fn acknowledge(&mut self, id: &str, revision: u64, dismiss: bool) -> bool {
        let episode = self
            .observations
            .iter_mut()
            .flat_map(|o| [&mut o.input, &mut o.completion])
            .flatten()
            .find(|episode| {
                episode.id == id
                    && episode.revision == revision
                    && !episode.read
                    && (episode.kind == AttentionKind::Completion
                        || (dismiss && !episode.available))
            });
        let Some(episode) = episode else {
            return false;
        };
        episode.read = true;
        self.prune_retired();
        true
    }

    pub fn invalidate_terminal(&mut self, terminal_id: &str) -> bool {
        let before = self.clone();
        for observation in &mut self.observations {
            if observation.source.terminal_id == terminal_id {
                make_unavailable(observation);
            }
        }
        self.prune_retired();
        *self != before
    }

    pub fn restart(&mut self) {
        for observation in &mut self.observations {
            make_unavailable(observation);
        }
        self.prune_retired();
    }

    pub fn record_loss(&mut self, terminal_id: &str, count: u64) {
        self.lost_transitions = self.lost_transitions.saturating_add(count);
        self.invalidate_terminal(terminal_id);
    }
}

fn valid_source(source: &AttentionSource) -> bool {
    [
        &source.project_id,
        &source.terminal_id,
        &source.attachment_id,
    ]
    .into_iter()
    .all(|id| !id.is_empty() && id.len() <= 256)
        && source
            .conversation
            .as_ref()
            .is_none_or(ConversationId::is_valid)
}

fn same_attachment(a: &AttentionSource, b: &AttentionSource) -> bool {
    a.terminal_id == b.terminal_id
        && a.attachment_id == b.attachment_id
        && a.generation == b.generation
        && a.conversation == b.conversation
}

fn same_completion_source(a: &AttentionSource, b: &AttentionSource) -> bool {
    match (&a.conversation, &b.conversation) {
        (Some(a), Some(b)) => a == b,
        _ => same_attachment(a, b),
    }
}

fn make_unavailable(observation: &mut AttentionObservation) {
    observation.lifecycle = None;
    for episode in [&mut observation.input, &mut observation.completion]
        .into_iter()
        .flatten()
    {
        episode.available = false;
        episode.terminal_available = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source() -> AttentionSource {
        AttentionSource {
            project_id: "p".into(),
            terminal_id: "t".into(),
            attachment_id: "boot".into(),
            generation: 1,
            conversation: None,
        }
    }
    fn report(state: &mut AttentionState, lifecycle: AgentLifecycle, n: u64) {
        state.record(
            source(),
            Some(&AgentStatus::new(lifecycle)),
            n,
            format!("00000000-0000-0000-0000-{n:012x}"),
        );
    }
    #[test]
    fn cycles_dedup_and_stale_acknowledgment() {
        let mut state = AttentionState::default();
        report(&mut state, AgentLifecycle::Done, 1);
        let old = state.episodes()[0].clone();
        report(&mut state, AgentLifecycle::Done, 2);
        assert_eq!(state.episodes()[0], old);
        assert!(state.acknowledge(&old.id, old.revision, false));
        report(&mut state, AgentLifecycle::Done, 3);
        assert!(state.episodes().is_empty());
        report(&mut state, AgentLifecycle::Working, 4);
        report(&mut state, AgentLifecycle::Done, 5);
        assert!(!state.acknowledge(&old.id, old.revision, false));
        assert_eq!(state.episodes().len(), 1);
    }
    #[test]
    fn unavailable_is_not_answered_and_summary_revision_is_guarded() {
        let mut state = AttentionState::default();
        report(&mut state, AgentLifecycle::Blocked, 1);
        let old = state.episodes()[0].clone();
        assert!(!state.acknowledge(&old.id, old.revision, true));
        let mut changed = AgentStatus::new(AgentLifecycle::Blocked);
        changed.custom = Some("different question".into());
        state.record(
            source(),
            Some(&changed),
            2,
            "00000000-0000-0000-0000-000000000002".into(),
        );
        state.restart();
        assert!(!state.acknowledge(&old.id, old.revision, true));
        let pending = state.episodes()[0].clone();
        assert!(!pending.available);
        assert!(state.acknowledge(&pending.id, pending.revision, true));
        report(&mut state, AgentLifecycle::Blocked, 3);
        assert_eq!(state.episodes().len(), 1);
        report(&mut state, AgentLifecycle::Idle, 4);
        assert!(state.episodes().is_empty());
    }

    #[test]
    fn clear_keeps_the_original_terminal_reachable_until_owner_invalidation() {
        for lifecycle in [AgentLifecycle::Done, AgentLifecycle::Blocked] {
            let mut state = AttentionState::default();
            report(&mut state, lifecycle, 1);
            let original = state.episodes()[0].clone();
            state.record(source(), None, 2, conversation(2).session_id);
            let cleared = state.episodes()[0].clone();
            assert_eq!(cleared.id, original.id);
            assert_eq!(cleared.revision, original.revision);
            assert!(!cleared.available);
            assert!(cleared.can_open_terminal());
            assert!(!cleared.read);

            state.invalidate_terminal("t");
            let retired = state.episodes()[0].clone();
            assert!(!retired.can_open_terminal());
            assert!(state.acknowledge(&retired.id, retired.revision, true));
        }
    }

    #[test]
    fn identity_replacement_and_restart_do_not_reveal_old_results() {
        let mut state = AttentionState::default();
        report(&mut state, AgentLifecycle::Done, 1);
        state.record(
            AttentionSource {
                attachment_id: "replacement".into(),
                ..source()
            },
            None,
            2,
            conversation(2).session_id,
        );
        assert!(!state.episodes()[0].can_open_terminal());

        let mut state = AttentionState::default();
        report(&mut state, AgentLifecycle::Done, 1);
        state.record(source(), None, 2, conversation(2).session_id);
        state.restart();
        assert!(!state.episodes()[0].can_open_terminal());
    }

    #[test]
    fn older_snapshots_only_allow_reveal_for_available_episodes() {
        let mut state = AttentionState::default();
        report(&mut state, AgentLifecycle::Done, 1);
        let mut value = serde_json::to_value(&state.episodes()[0]).unwrap();
        value.as_object_mut().unwrap().remove("terminal_available");
        let live: AttentionEpisode = serde_json::from_value(value.clone()).unwrap();
        assert!(live.can_open_terminal());
        value["available"] = false.into();
        let unavailable: AttentionEpisode = serde_json::from_value(value).unwrap();
        assert!(!unavailable.can_open_terminal());
    }

    fn conversation(n: u64) -> ConversationId {
        ConversationId {
            agent: "claude-code".into(),
            session_id: format!("00000000-0000-0000-0000-{n:012x}"),
        }
    }

    #[test]
    fn identity_switch_clear_does_not_retarget_pending_question() {
        let mut state = AttentionState::default();
        let a = AttentionSource {
            conversation: Some(conversation(1)),
            ..source()
        };
        let b = AttentionSource {
            conversation: Some(conversation(2)),
            ..source()
        };
        state.record(
            a.clone(),
            Some(&AgentStatus::new(AgentLifecycle::Blocked)),
            1,
            conversation(11).session_id,
        );
        state.record(b.clone(), None, 2, conversation(12).session_id);
        state.record(
            b,
            Some(&AgentStatus::new(AgentLifecycle::Working)),
            3,
            conversation(13).session_id,
        );
        let pending = &state.episodes()[0];
        assert_eq!(pending.source, a);
        assert!(!pending.available);
        assert!(!pending.read);
    }

    #[test]
    fn resumed_conversation_supersedes_completion_across_attachments() {
        let mut state = AttentionState::default();
        let a = AttentionSource {
            conversation: Some(conversation(1)),
            ..source()
        };
        state.record(
            a.clone(),
            Some(&AgentStatus::new(AgentLifecycle::Done)),
            1,
            conversation(11).session_id,
        );
        let old = state.episodes()[0].clone();
        state.record(a.clone(), None, 2, conversation(12).session_id);
        assert_eq!(state.episodes()[0].id, old.id);
        let b = AttentionSource {
            attachment_id: "restart".into(),
            terminal_id: "new-pty".into(),
            generation: 2,
            ..a
        };
        state.record(
            b.clone(),
            Some(&AgentStatus::new(AgentLifecycle::Done)),
            3,
            conversation(13).session_id,
        );
        assert_eq!(state.episodes().len(), 1);
        assert_eq!(state.episodes()[0].source, b);
        assert!(!state.acknowledge(&old.id, old.revision, false));
    }

    #[test]
    fn concurrent_attachments_isolate_input_and_deduplicate_superseded_completions() {
        let mut state = AttentionState::default();
        let a = AttentionSource {
            conversation: Some(conversation(1)),
            ..source()
        };
        let b = AttentionSource {
            terminal_id: "other".into(),
            ..a.clone()
        };
        let report = |state: &mut AttentionState, source: &AttentionSource, lifecycle, n| {
            state.record(
                source.clone(),
                Some(&AgentStatus::new(lifecycle)),
                n,
                conversation(n + 10).session_id,
            );
        };
        report(&mut state, &a, AgentLifecycle::Blocked, 1);
        report(&mut state, &b, AgentLifecycle::Blocked, 2);
        state.record(a.clone(), None, 3, conversation(13).session_id);
        let b_block = state
            .episodes()
            .into_iter()
            .find(|e| e.source == b)
            .unwrap();
        assert!(b_block.available);
        report(&mut state, &a, AgentLifecycle::Working, 4);
        assert_eq!(state.episodes(), vec![b_block]);
        report(&mut state, &b, AgentLifecycle::Done, 5);
        let old = state.episodes()[0].clone();
        report(&mut state, &a, AgentLifecycle::Done, 6);
        let latest = state.episodes()[0].clone();
        assert_eq!(state.episodes().len(), 1);
        assert!(!state.acknowledge(&old.id, old.revision, false));
        report(&mut state, &b, AgentLifecycle::Done, 7);
        assert_eq!(state.episodes(), vec![latest.clone()]);
        assert!(state.acknowledge(&latest.id, latest.revision, false));
        report(&mut state, &a, AgentLifecycle::Done, 8);
        report(&mut state, &b, AgentLifecycle::Done, 9);
        assert!(state.episodes().is_empty());
    }

    #[test]
    fn persisted_validation_rebuckets_valid_completion_and_rejects_unrelated_identity() {
        let mut state = AttentionState::default();
        let a = AttentionSource {
            conversation: Some(conversation(1)),
            ..source()
        };
        state.record(
            a.clone(),
            Some(&AgentStatus::new(AgentLifecycle::Done)),
            1,
            conversation(11).session_id,
        );
        let completed = state.episodes()[0].clone();
        state.observations[0].input = state.observations[0].completion.take();
        state.observations[0].source.terminal_id = "resumed".into();
        let mut unrelated = completed.clone();
        unrelated.id = conversation(12).session_id;
        unrelated.kind = AttentionKind::InputNeeded;
        unrelated.source.conversation = Some(conversation(2));
        state.observations[0].completion = Some(unrelated);
        state.validate_persisted();
        assert_eq!(state.episodes().len(), 1);
        assert_eq!(state.episodes()[0].id, completed.id);
        assert_eq!(state.episodes()[0].source, a);
        assert_eq!(state.episodes()[0].kind, AttentionKind::Completion);
        assert!(state.observations[0].input.is_none());
        state.record(
            a,
            Some(&AgentStatus::new(AgentLifecycle::Working)),
            3,
            conversation(13).session_id,
        );
        assert_eq!(state.episodes()[0].id, completed.id);
    }

    #[test]
    fn persisted_duplicates_reconcile_to_latest_completion_and_revision() {
        let mut state = AttentionState::default();
        let a = AttentionSource {
            conversation: Some(conversation(1)),
            ..source()
        };
        state.record(
            a.clone(),
            Some(&AgentStatus::new(AgentLifecycle::Done)),
            1,
            conversation(11).session_id,
        );
        let old = state.observations[0].clone();
        let b = AttentionSource {
            terminal_id: "other".into(),
            ..a.clone()
        };
        state.record(
            b.clone(),
            Some(&AgentStatus::new(AgentLifecycle::Done)),
            2,
            conversation(12).session_id,
        );
        let latest = state
            .observations
            .iter()
            .find(|o| o.source == b)
            .unwrap()
            .clone();
        state
            .observations
            .extend([old.clone(), latest.clone(), old]);
        state.validate_persisted();
        assert_eq!(state.episodes().len(), 1);
        assert_eq!(state.episodes()[0].source, b);
        let mut acknowledged = latest;
        acknowledged.completion.as_mut().unwrap().revision = 2;
        acknowledged.completion.as_mut().unwrap().read = true;
        state.observations.push(acknowledged);
        state.validate_persisted();
        assert!(state.episodes().is_empty());
        state.record(
            a,
            Some(&AgentStatus::new(AgentLifecycle::Done)),
            3,
            conversation(13).session_id,
        );
        let mut conflicting = state
            .observations
            .iter()
            .find(|o| o.completion.is_some())
            .unwrap()
            .clone();
        conflicting.source.conversation = Some(conversation(2));
        conflicting.completion.as_mut().unwrap().source.conversation = Some(conversation(2));
        state.observations.push(conflicting);
        state.validate_persisted();
        assert!(state.episodes().is_empty());
    }

    #[test]
    fn stale_generation_cannot_resolve_current_block() {
        let mut state = AttentionState::default();
        let current = AttentionSource {
            generation: 2,
            ..source()
        };
        state.record(
            current.clone(),
            Some(&AgentStatus::new(AgentLifecycle::Blocked)),
            1,
            conversation(1).session_id,
        );
        assert!(!state.record(
            source(),
            Some(&AgentStatus::new(AgentLifecycle::Working)),
            2,
            conversation(2).session_id
        ));
        assert_eq!(state.episodes().len(), 1);
        assert!(state.episodes()[0].available);
        state.record(
            current,
            Some(&AgentStatus::new(AgentLifecycle::Working)),
            3,
            conversation(3).session_id,
        );
        assert!(state.episodes().is_empty());
    }

    #[test]
    fn anonymous_retention_is_bounded_and_exposes_loss() {
        let mut state = AttentionState::default();
        for n in 0..200 {
            let source = AttentionSource {
                terminal_id: format!("t{n}"),
                ..source()
            };
            state.record(
                source.clone(),
                Some(&AgentStatus::new(AgentLifecycle::Blocked)),
                n,
                conversation(n).session_id,
            );
            state.invalidate_terminal(&source.terminal_id);
        }
        state.restart();
        assert_eq!(state.observations.len(), MAX_UNAVAILABLE_ANONYMOUS_SOURCES);
        assert_eq!(state.lost_transitions, 72);
    }
}
