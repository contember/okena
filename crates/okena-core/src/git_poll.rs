use crate::api::ActionRequest;

/// External wake-up request for git / GitHub status polling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitPollTrigger {
    pub project_id: Option<String>,
    pub poll_github: bool,
    pub invalidate_github: bool,
}

impl GitPollTrigger {
    pub fn branch_change(project_id: String) -> Self {
        Self {
            project_id: Some(project_id),
            poll_github: true,
            invalidate_github: true,
        }
    }

    pub fn project_visible(project_id: String) -> Self {
        Self {
            project_id: Some(project_id),
            poll_github: true,
            invalidate_github: false,
        }
    }

    pub fn visibility_changed() -> Self {
        Self {
            project_id: None,
            poll_github: false,
            invalidate_github: false,
        }
    }
}

/// Map an action to the git-poll wake-up it should trigger (if any).
///
/// Shared by both daemons so the same actions always drive the same immediate
/// refresh: the dedicated `okena-daemon` (`DaemonCore`) and the single-binary
/// `okena --headless` (`HeadlessApp`) both call this after a successful action.
///
/// - Branch checkout invalidates the cached PR/CI (they belong to the old
///   branch) and re-polls `gh`.
/// - Requesting git status, or showing a project in the overview, marks the
///   project visible so `gh` is fetched when it has no cached PR/CI yet.
pub fn git_poll_trigger_for_action(action: &ActionRequest) -> Option<GitPollTrigger> {
    match action {
        ActionRequest::GitCheckoutLocalBranch { project_id, .. }
        | ActionRequest::GitCheckoutRemoteBranch { project_id, .. }
        | ActionRequest::GitCreateAndCheckoutBranch { project_id, .. } => {
            Some(GitPollTrigger::branch_change(project_id.clone()))
        }
        ActionRequest::GitStatus { project_id }
        | ActionRequest::SetProjectShowInOverview {
            project_id,
            show: true,
            ..
        } => Some(GitPollTrigger::project_visible(project_id.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_checkout_creates_invalidating_trigger() {
        let trigger = git_poll_trigger_for_action(&ActionRequest::GitCheckoutLocalBranch {
            project_id: "p1".to_string(),
            branch: "feature".to_string(),
        })
        .expect("branch checkout creates trigger");
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(trigger.poll_github);
        assert!(trigger.invalidate_github);
    }

    #[test]
    fn visible_project_actions_create_non_invalidating_trigger() {
        let trigger = git_poll_trigger_for_action(&ActionRequest::SetProjectShowInOverview {
            project_id: "p1".to_string(),
            show: true,
            window: None,
        })
        .expect("showing a project creates trigger");
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(trigger.poll_github);
        assert!(!trigger.invalidate_github);

        let trigger = git_poll_trigger_for_action(&ActionRequest::GitStatus {
            project_id: "p1".to_string(),
        })
        .expect("requesting git status creates trigger");
        assert_eq!(trigger.project_id.as_deref(), Some("p1"));
        assert!(trigger.poll_github);
        assert!(!trigger.invalidate_github);

        assert!(
            git_poll_trigger_for_action(&ActionRequest::SetProjectShowInOverview {
                project_id: "p1".to_string(),
                show: false,
                window: None,
            })
            .is_none()
        );
    }
}
