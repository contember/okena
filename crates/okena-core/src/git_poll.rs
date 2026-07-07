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
