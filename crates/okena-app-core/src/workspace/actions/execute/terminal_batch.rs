use okena_terminal::TerminalsRegistry;
use okena_terminal::backend::{TerminalBackend, TerminalLaunchPlan};
use okena_terminal::terminal::{Terminal, TerminalSize};
use std::collections::HashSet;
use std::sync::Arc;

/// One reserved layout terminal that can be published before its PTY is launched.
#[derive(Clone)]
pub struct PreparedTerminalLaunch {
    pub(super) project_id: String,
    pub(super) layout_path: Vec<usize>,
    pub(super) terminal_id: String,
    pub(super) cwd: String,
    pub(super) launch_plan: TerminalLaunchPlan,
}

impl PreparedTerminalLaunch {
    pub fn new(
        project_id: String,
        layout_path: Vec<usize>,
        terminal_id: String,
        cwd: String,
        launch_plan: TerminalLaunchPlan,
    ) -> Self {
        Self {
            project_id,
            layout_path,
            terminal_id,
            cwd,
            launch_plan,
        }
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn layout_path(&self) -> &[usize] {
        &self.layout_path
    }

    pub fn terminal_id(&self) -> &str {
        &self.terminal_id
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn launch_plan(&self) -> &TerminalLaunchPlan {
        &self.launch_plan
    }
}

#[derive(Clone)]
struct PublishedTerminalOwner {
    terminal_id: String,
    terminal: Arc<Terminal>,
}

/// Exact registry owners published before a blocking materialization batch.
#[derive(Clone)]
pub struct PublishedTerminalOwners {
    terminals: TerminalsRegistry,
    owners: Vec<PublishedTerminalOwner>,
}

impl PublishedTerminalOwners {
    /// Release only IDs that still point at this batch's exact registry owner.
    pub fn release(&self, terminal_ids: &[String]) -> Vec<String> {
        let requested: HashSet<&str> = terminal_ids.iter().map(String::as_str).collect();
        let mut released = Vec::new();
        let mut registry = self.terminals.lock();
        for owner in &self.owners {
            if !requested.contains(owner.terminal_id.as_str()) {
                continue;
            }
            let still_owned = registry
                .get(&owner.terminal_id)
                .is_some_and(|terminal| Arc::ptr_eq(terminal, &owner.terminal));
            if still_owned {
                registry.remove(&owner.terminal_id);
                released.push(owner.terminal_id.clone());
            }
        }
        released
    }

    pub fn release_all(&self) -> Vec<String> {
        let terminal_ids = self
            .owners
            .iter()
            .map(|owner| owner.terminal_id.clone())
            .collect::<Vec<_>>();
        self.release(&terminal_ids)
    }
}

#[derive(Default)]
pub struct PreparedTerminalLaunchOutcome {
    pub failed_terminal_ids: Vec<String>,
    pub errors: Vec<String>,
}

/// Publish logical registry owners without launching a PTY.
pub fn publish_prepared_terminal_launches(
    launches: &[PreparedTerminalLaunch],
    terminals: &TerminalsRegistry,
    backend: &dyn TerminalBackend,
) -> Result<PublishedTerminalOwners, String> {
    let mut owners: Vec<PublishedTerminalOwner> = Vec::with_capacity(launches.len());
    let transport = backend.transport();
    let mut registry = terminals.lock();
    for launch in launches {
        if registry.contains_key(&launch.terminal_id) {
            for owner in &owners {
                registry.remove(&owner.terminal_id);
            }
            return Err(format!(
                "terminal reservation already exists: {}",
                launch.terminal_id
            ));
        }
        let terminal = Arc::new(Terminal::new(
            launch.terminal_id.clone(),
            TerminalSize::default(),
            transport.clone(),
            launch.cwd.clone(),
        ));
        registry.insert(launch.terminal_id.clone(), terminal.clone());
        owners.push(PublishedTerminalOwner {
            terminal_id: launch.terminal_id.clone(),
            terminal,
        });
    }
    drop(registry);
    Ok(PublishedTerminalOwners {
        terminals: terminals.clone(),
        owners,
    })
}

/// Run the blocking reconnect portion after logical owners are visible.
pub fn materialize_prepared_terminal_launches(
    launches: &[PreparedTerminalLaunch],
    backend: &dyn TerminalBackend,
) -> PreparedTerminalLaunchOutcome {
    let mut outcome = PreparedTerminalLaunchOutcome::default();
    for launch in launches {
        match backend.reconnect_terminal_with_plan(
            &launch.terminal_id,
            &launch.cwd,
            &launch.launch_plan,
        ) {
            Ok(id) if id == launch.terminal_id => {}
            Ok(id) => {
                backend.kill(&id);
                outcome.failed_terminal_ids.push(launch.terminal_id.clone());
                outcome.errors.push(format!(
                    "backend returned unexpected terminal id {id} for {}",
                    launch.terminal_id
                ));
            }
            Err(error) => {
                outcome.failed_terminal_ids.push(launch.terminal_id.clone());
                outcome.errors.push(format!(
                    "failed to materialize terminal {}: {error}",
                    launch.terminal_id
                ));
            }
        }
    }
    outcome
}

/// Tear down a stale completion without deleting a newer registry owner.
pub fn cleanup_stale_prepared_terminal_launches(
    owners: &PublishedTerminalOwners,
    backend: &dyn TerminalBackend,
) {
    for terminal_id in owners.release_all() {
        backend.kill(&terminal_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_terminal::shell_config::ShellType;
    use okena_terminal::terminal::TerminalTransport;
    use std::sync::Mutex;

    struct TestTransport;

    impl TerminalTransport for TestTransport {
        fn send_input(&self, _terminal_id: &str, _data: &[u8]) {}

        fn resize(&self, _terminal_id: &str, _cols: u16, _rows: u16) {}

        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    struct TestBackend {
        killed: Mutex<Vec<String>>,
        transport: Arc<TestTransport>,
    }

    impl TestBackend {
        fn new() -> Self {
            Self {
                killed: Mutex::new(Vec::new()),
                transport: Arc::new(TestTransport),
            }
        }
    }

    impl TerminalBackend for TestBackend {
        fn transport(&self) -> Arc<dyn TerminalTransport> {
            self.transport.clone()
        }

        fn create_terminal(
            &self,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            unreachable!("test uses reserved reconnects")
        }

        fn reconnect_terminal(
            &self,
            terminal_id: &str,
            _cwd: &str,
            _shell: Option<&ShellType>,
        ) -> anyhow::Result<String> {
            Ok(terminal_id.to_string())
        }

        fn kill(&self, terminal_id: &str) {
            self.killed
                .lock()
                .expect("killed terminals lock")
                .push(terminal_id.to_string());
        }

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

    fn launch(id: &str) -> PreparedTerminalLaunch {
        PreparedTerminalLaunch::new(
            "project".to_string(),
            Vec::new(),
            id.to_string(),
            "/tmp".to_string(),
            TerminalLaunchPlan::for_shell(ShellType::Default),
        )
    }

    #[test]
    fn publication_rolls_back_on_reservation_collision() {
        let backend = TestBackend::new();
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let existing = Arc::new(Terminal::new(
            "collision".to_string(),
            TerminalSize::default(),
            backend.transport(),
            "/tmp".to_string(),
        ));
        terminals
            .lock()
            .insert("collision".to_string(), existing.clone());

        let error = publish_prepared_terminal_launches(
            &[launch("reserved"), launch("collision")],
            &terminals,
            &backend,
        )
        .err()
        .expect("collision rejects batch");

        assert!(error.contains("already exists"));
        assert!(!terminals.lock().contains_key("reserved"));
        assert!(Arc::ptr_eq(
            terminals.lock().get("collision").expect("existing owner"),
            &existing
        ));
    }

    #[test]
    fn stale_cleanup_does_not_remove_or_kill_replacement_owner() {
        let backend = TestBackend::new();
        let terminals: TerminalsRegistry = Arc::new(Default::default());
        let owners =
            publish_prepared_terminal_launches(&[launch("reserved")], &terminals, &backend)
                .expect("publish owner");
        let replacement = Arc::new(Terminal::new(
            "reserved".to_string(),
            TerminalSize::default(),
            backend.transport(),
            "/replacement".to_string(),
        ));
        terminals
            .lock()
            .insert("reserved".to_string(), replacement.clone());

        cleanup_stale_prepared_terminal_launches(&owners, &backend);

        assert!(
            backend
                .killed
                .lock()
                .expect("killed terminals lock")
                .is_empty()
        );
        assert!(Arc::ptr_eq(
            terminals.lock().get("reserved").expect("replacement owner"),
            &replacement
        ));
    }
}
