//! Terminal pane action handlers.

use crate::ActionDispatch;
use okena_core::api::ActionRequest;
use okena_terminal::shell_config::ShellType;
use okena_workspace::state::SplitDirection;
use gpui::*;

use super::TerminalPane;

impl<D: ActionDispatch + Send + Sync> TerminalPane<D> {
    pub(super) fn handle_split(&mut self, direction: SplitDirection, cx: &mut Context<Self>) {
        if let Some(ref dispatcher) = self.action_dispatcher {
            dispatcher.split_terminal(&self.project_id, &self.layout_path, direction, cx);
        }
    }

    pub(super) fn handle_add_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(ref dispatcher) = self.action_dispatcher {
            dispatcher.add_tab(&self.project_id, &self.layout_path, false, cx);
        }
    }

    pub(super) fn handle_close(&mut self, cx: &mut Context<Self>) {
        if let Some(terminal_id) = self.terminal_id.clone() {
            let action = ActionRequest::CloseTerminal {
                project_id: self.project_id.clone(),
                terminal_id,
            };
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(action, cx);
            }
        }
    }

    pub(super) fn handle_minimize(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id {
            let action = ActionRequest::ToggleMinimized {
                project_id: self.project_id.clone(),
                terminal_id: terminal_id.clone(),
            };
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(action, cx);
            }
        }
    }

    pub(super) fn handle_fullscreen(&mut self, cx: &mut Context<Self>) {
        if let Some(ref id) = self.terminal_id {
            let action = ActionRequest::SetFullscreen {
                project_id: self.project_id.clone(),
                terminal_id: Some(id.clone()),
            };
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(action, cx);
            }
        }
    }

    pub(super) fn handle_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if let Some(text) = terminal.get_selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    pub(super) fn handle_paste(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.terminal.clone() else {
            return;
        };
        let Some(clipboard_item) = cx.read_from_clipboard() else {
            return;
        };

        if let Some(text) = clipboard_item.text() {
            terminal.send_paste(&text);
            return;
        }

        let image = clipboard_item.entries().iter().find_map(|e| match e {
            ClipboardEntry::Image(img) => Some(img.clone()),
            _ => None,
        });
        let Some(image) = image else { return };

        let filename = paste_filename(&image);

        // WSL fast path: write the image into the distro's own /tmp via the
        // `\\wsl$\<distro>` UNC mount, then try to inject into its
        // Wayland/X11 clipboard so Claude attaches `[Image #N]` rather than
        // pasting the path as text. Falls back to the bracketed path when
        // wl-copy / xclip aren't installed.
        #[cfg(target_os = "windows")]
        {
            let shell = self.resolved_shell(cx);
            if let Some(distro) = wsl_distro(&shell) {
                let unc = format!(r"\\wsl$\{}\tmp\{}", distro, filename);
                if std::fs::write(&unc, &image.bytes).is_ok() {
                    let wsl_path = format!("/tmp/{}", filename);
                    if inject_into_wsl_clipboard(&distro, &wsl_path, image.format.mime_type()) {
                        terminal.send_bytes(b"\x16");
                    } else {
                        terminal.send_paste(&wsl_path);
                    }
                    return;
                }
                log::warn!("WSL UNC write to {} failed; falling back to /mnt/c", unc);
            }

            let Some(path) = write_paste_image_to_temp(&image, &filename) else { return };
            let path_str = if matches!(shell, ShellType::Wsl { .. }) {
                okena_terminal::shell_config::windows_path_to_wsl(&path.to_string_lossy())
            } else {
                path.to_string_lossy().into_owned()
            };
            terminal.send_paste(&path_str);
        }

        #[cfg(not(target_os = "windows"))]
        {
            let Some(path) = write_paste_image_to_temp(&image, &filename) else { return };
            terminal.send_paste(&path.to_string_lossy());
        }
    }

    fn resolved_shell(&self, cx: &mut Context<Self>) -> ShellType {
        let settings = crate::terminal_view_settings(cx);
        let ws = self.workspace.read(cx);
        self.shell_type.clone().resolve_default(
            ws.project(&self.project_id).and_then(|p| p.default_shell.as_ref()),
            &settings.default_shell,
        )
    }

    pub(super) fn handle_jump_prev_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if terminal.jump_to_prompt_above() {
                cx.notify();
            }
        }
    }

    pub(super) fn handle_jump_next_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            if terminal.jump_to_prompt_below() {
                cx.notify();
            }
        }
    }

    pub(super) fn handle_file_drop(&mut self, paths: &ExternalPaths, _cx: &mut Context<Self>) {
        let Some(ref terminal) = self.terminal else {
            return;
        };

        for path in paths.paths() {
            let escaped_path = Self::shell_escape_path(path);
            terminal.send_input(&format!("{} ", escaped_path));
        }
    }

    pub(super) fn shell_escape_path(path: &std::path::Path) -> String {
        let path_str = path.to_string_lossy();
        let mut escaped = String::with_capacity(path_str.len() * 2);

        for c in path_str.chars() {
            match c {
                ' ' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | '`' | '$' | '&' | '|'
                | ';' | '<' | '>' | '*' | '?' | '!' | '#' | '~' | '\\' => {
                    escaped.push('\\');
                    escaped.push(c);
                }
                _ => escaped.push(c),
            }
        }

        escaped
    }
}

fn paste_filename(image: &Image) -> String {
    let ext = match image.format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
    };
    format!("okena-paste-{:016x}.{}", image.id, ext)
}

fn write_paste_image_to_temp(image: &Image, filename: &str) -> Option<std::path::PathBuf> {
    let path = std::env::temp_dir().join(filename);
    if let Err(e) = std::fs::write(&path, &image.bytes) {
        log::error!("Failed to write pasted image to {}: {}", path.display(), e);
        return None;
    }
    Some(path)
}

/// Resolve the WSL distro to use for a shell. Returns `None` when the shell is
/// not WSL. For `Wsl { distro: None }` (i.e. the user picked "WSL Default"),
/// queries `wsl.exe -l -q` once and caches the first entry — staleness only
/// matters if the user installs/uninstalls a distro between paste attempts.
#[cfg(target_os = "windows")]
fn wsl_distro(shell: &ShellType) -> Option<String> {
    use std::sync::OnceLock;
    let ShellType::Wsl { distro } = shell else { return None };
    if let Some(d) = distro {
        return Some(d.clone());
    }
    static DEFAULT: OnceLock<Option<String>> = OnceLock::new();
    DEFAULT
        .get_or_init(|| okena_terminal::shell_config::detect_wsl_distros().into_iter().next())
        .clone()
}

/// Place an image onto the WSL distro's Wayland/X11 clipboard so the running
/// TUI's own Ctrl+V handler picks it up. Returns `true` only when one of the
/// helpers actually succeeded — caller should fall back to bracketed-pasting
/// the path otherwise. Requires `wl-clipboard` (preferred — daemonises
/// cleanly under WSLg) or `xclip` to be installed in the distro.
#[cfg(target_os = "windows")]
fn inject_into_wsl_clipboard(distro: &str, wsl_path: &str, mime: &str) -> bool {
    // wl-copy daemonises after consuming stdin, so the wsl.exe call returns
    // promptly. xclip needs `setsid ... &` to detach from our subprocess.
    let cmd = format!(
        r#"if command -v wl-copy >/dev/null 2>&1; then \
              wl-copy --type {mime} < "{path}"; \
           elif command -v xclip >/dev/null 2>&1; then \
              setsid xclip -selection clipboard -t {mime} -i "{path}" </dev/null >/dev/null 2>&1 & \
              disown; \
              sleep 0.05; \
           else \
              exit 127; \
           fi"#,
        mime = mime,
        path = wsl_path,
    );
    run_in_wsl(distro, &cmd)
}

#[cfg(target_os = "windows")]
fn run_in_wsl(distro: &str, cmd: &str) -> bool {
    let mut command = okena_core::process::command("wsl.exe");
    command
        .args(["-d", distro, "--", "bash", "-c", cmd])
        .stdin(std::process::Stdio::null());
    match okena_core::process::safe_output_with_timeout(
        &mut command,
        std::time::Duration::from_secs(3),
    ) {
        Ok(output) => output.status.success(),
        Err(e) => {
            log::warn!("wsl.exe -d {} failed: {}", distro, e);
            false
        }
    }
}
