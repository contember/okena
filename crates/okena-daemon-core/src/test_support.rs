use std::sync::Arc;

use okena_state::WorkspaceData;
use okena_terminal::backend::TerminalBackend;
use okena_terminal::shell_config::ShellType;
use okena_terminal::terminal::TerminalTransport;
use okena_workspace::persistence::AppSettings;

pub(crate) struct StubTransport;

impl TerminalTransport for StubTransport {
    fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}
    fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}
    fn uses_mouse_backend(&self) -> bool {
        false
    }
}

pub(crate) struct StubBackend;

impl TerminalBackend for StubBackend {
    fn transport(&self) -> Arc<dyn TerminalTransport> {
        Arc::new(StubTransport)
    }

    fn create_terminal(&self, _cwd: &str, _shell: Option<&ShellType>) -> anyhow::Result<String> {
        anyhow::bail!("stub backend: create_terminal not supported")
    }

    fn reconnect_terminal(
        &self,
        _terminal_id: &str,
        _cwd: &str,
        _shell: Option<&ShellType>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("stub backend: reconnect_terminal not supported")
    }

    fn kill(&self, _terminal_id: &str) {}

    fn capture_buffer(&self, _terminal_id: &str) -> Option<std::path::PathBuf> {
        None
    }

    fn supports_buffer_capture(&self) -> bool {
        false
    }

    fn is_remote(&self) -> bool {
        false
    }

    fn get_shell_pid(&self, _terminal_id: &str) -> Option<u32> {
        None
    }

    fn get_service_pids(&self, _terminal_id: &str) -> Vec<u32> {
        Vec::new()
    }
}

pub(crate) fn empty_workspace_data() -> WorkspaceData {
    WorkspaceData {
        version: 1,
        projects: Vec::new(),
        project_order: Vec::new(),
        folders: Vec::new(),
        service_panel_heights: Default::default(),
        hook_panel_heights: Default::default(),
        main_window: Default::default(),
        extra_windows: Vec::new(),
    }
}

pub(crate) fn default_settings() -> AppSettings {
    serde_json::from_value::<AppSettings>(serde_json::json!({})).expect("defaults")
}
