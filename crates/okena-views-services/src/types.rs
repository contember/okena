use okena_services::manager::ServiceStatus;

/// Unified snapshot of service state — used for rendering.
/// Created from either local ServiceManager or remote ProjectData.
#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    pub name: String,
    pub status: ServiceStatus,
    pub terminal_id: Option<String>,
    pub ports: Vec<u16>,
    pub is_docker: bool,
    /// Docker service not listed in okena.yaml — shown in "Other" section.
    pub is_extra: bool,
}

/// Compute the status dot color for a given ServiceStatus.
pub fn status_color(status: &ServiceStatus, t: &okena_ui::theme::ThemeColors) -> u32 {
    match status {
        ServiceStatus::Running => t.term_green,
        ServiceStatus::Crashed { .. } => t.term_red,
        ServiceStatus::Stopped => t.text_muted,
        ServiceStatus::Starting | ServiceStatus::Restarting => t.term_yellow,
    }
}

/// Compute the status label string for a given ServiceStatus.
pub fn status_label(status: &ServiceStatus) -> &'static str {
    match status {
        ServiceStatus::Running => "running",
        ServiceStatus::Crashed { exit_code } => {
            if exit_code.is_some() { "exited" } else { "crashed" }
        }
        ServiceStatus::Stopped => "stopped",
        ServiceStatus::Starting => "starting",
        ServiceStatus::Restarting => "restarting",
    }
}
