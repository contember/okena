//! Terminal pane action handlers.

use crate::{ActionDispatch, RemotePasteFile};
use gpui::*;
use okena_core::api::ActionRequest;
use okena_terminal::shell_config::ShellType;
use okena_workspace::state::SplitDirection;
use okena_workspace::toast::{Toast, ToastManager};

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

    pub(super) fn handle_export_buffer(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal_id) = self.terminal_id
            && let Some(ref dispatcher) = self.action_dispatcher
        {
            dispatcher.export_buffer_to_clipboard(terminal_id, cx);
        }
    }

    pub(super) fn handle_detach(&mut self, cx: &mut Context<Self>) {
        let project_id = self.project_id.clone();
        let layout_path = self.layout_path.clone();
        self.workspace.update(cx, |ws, cx| {
            ws.detach_terminal(&project_id, &layout_path, cx);
        });
    }

    /// Toggle the pane's unread mark — the bell indicator the shell raises on
    /// BEL, set by hand so a pane can be flagged to come back to.
    pub(super) fn handle_toggle_unread(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal {
            terminal.toggle_unread();
            cx.notify();
            // The mark shows in four places, and the sidebar is a `.cached()`
            // sibling this pane's notify never reaches. One keypress is far
            // too rare for the cost of bypassing the caches to matter.
            cx.refresh_windows();
        }
    }

    pub(super) fn handle_fullscreen(&mut self, cx: &mut Context<Self>) {
        if let Some(ref id) = self.terminal_id {
            let action = ActionRequest::SetFullscreen {
                project_id: self.project_id.clone(),
                terminal_id: Some(id.clone()),
                window: None,
            };
            if let Some(ref dispatcher) = self.action_dispatcher {
                dispatcher.dispatch(action, cx);
            }
        }
    }

    pub(super) fn handle_copy(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal
            && let Some(text) = terminal.get_selected_text()
        {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    /// Open the annotate composer over the current selection. The keyboard path
    /// to the same thing the context menu offers — and the only one that works
    /// while an app holds the mouse grabbed.
    pub(super) fn handle_annotate_selection(&mut self, cx: &mut Context<Self>) {
        let Some(terminal_id) = self.terminal_id.clone() else {
            return;
        };
        let has_selection = self
            .terminal
            .as_ref()
            .and_then(|t| t.get_selected_text())
            .is_some_and(|text| !text.trim().is_empty());
        if !has_selection {
            return;
        }
        let position = self
            .content
            .read(cx)
            .selection_anchor()
            .unwrap_or_else(|| gpui::point(gpui::px(120.0), gpui::px(120.0)));
        let project_id = self.project_id.clone();
        self.request_broker.update(cx, |broker, cx| {
            broker.push_overlay_request(
                okena_workspace::requests::OverlayRequest::Project(
                    okena_workspace::requests::ProjectOverlay {
                        project_id,
                        kind: okena_workspace::requests::ProjectOverlayKind::AnnotateSelection {
                            terminal_id,
                            position,
                        },
                    },
                ),
                cx,
            );
        });
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

        // Remote terminals: the temp file the terminal's process reads lives on
        // the *server's* filesystem, not ours. Upload the bytes so the server
        // writes the file and bracketed-pastes its path; the client-local temp
        // paths below would hand the server a path that doesn't exist on it.
        if let Some(ref dispatcher) = self.action_dispatcher
            && !dispatcher.shares_local_filesystem()
            && let Some(ref terminal_id) = self.terminal_id
        {
            dispatcher.upload_remote_paste_image(
                terminal_id,
                image.format.mime_type(),
                image.bytes.clone(),
                cx,
            );
            return;
        }

        let filename = paste_filename(&image);

        // WSL fast path: write the image into the distro's own /tmp via the
        // `\\wsl$\<distro>` UNC mount, then try to inject into its
        // Wayland/X11 clipboard so Claude attaches `[Image #N]` rather than
        // pasting the path as text. Falls back to the bracketed path when
        // wl-copy / xclip aren't installed.
        #[cfg(target_os = "windows")]
        {
            let settings = crate::terminal_view_settings(cx);
            let ws = self.workspace.read(cx);
            let shell = self.shell_type.clone().resolve_default(
                ws.project(&self.project_id)
                    .and_then(|p| p.default_shell.as_ref()),
                &settings.default_shell,
            );
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

            let Some(path) = write_paste_image_to_temp(&image, &filename) else {
                return;
            };
            let path_str = if matches!(shell, ShellType::Wsl { .. }) {
                okena_terminal::shell_config::windows_path_to_wsl(&path.to_string_lossy())
            } else {
                path.to_string_lossy().into_owned()
            };
            terminal.send_paste(&path_str);
        }

        #[cfg(not(target_os = "windows"))]
        {
            let Some(path) = write_paste_image_to_temp(&image, &filename) else {
                return;
            };
            terminal.send_paste(&path.to_string_lossy());
        }
    }

    pub(super) fn handle_jump_prev_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal
            && terminal.jump_to_prompt_above()
        {
            cx.notify();
        }
    }

    pub(super) fn handle_jump_next_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal
            && terminal.jump_to_prompt_below()
        {
            cx.notify();
        }
    }

    pub(super) fn handle_jump_prev_failed(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal
            && terminal.jump_to_prev_failed_command()
        {
            cx.notify();
        }
    }

    pub(super) fn handle_jump_next_failed(&mut self, cx: &mut Context<Self>) {
        if let Some(ref terminal) = self.terminal
            && terminal.jump_to_next_failed_command()
        {
            cx.notify();
        }
    }

    pub(super) fn handle_file_drop(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        let Some(ref terminal) = self.terminal else {
            return;
        };

        let Some(ref dispatcher) = self.action_dispatcher else {
            return;
        };

        if !dispatcher.shares_local_filesystem() {
            let Some(terminal_id) = self.terminal_id.clone() else {
                return;
            };
            let dispatcher = dispatcher.clone();
            let paths = paths.paths().to_vec();
            cx.spawn(async move |_this, cx| {
                let files = smol::unblock(move || {
                    let mut files = Vec::new();
                    let mut total_bytes = 0usize;
                    for path in paths.into_iter().take(20) {
                        match read_remote_paste_file(&path) {
                            Ok(file)
                                if total_bytes + file.bytes.len()
                                    <= REMOTE_FILE_UPLOAD_LIMIT as usize =>
                            {
                                total_bytes += file.bytes.len();
                                files.push(file);
                            }
                            Ok(_) => {
                                log::error!(
                                    "Cannot upload dropped files: combined size exceeds {} MiB",
                                    REMOTE_FILE_UPLOAD_LIMIT / 1024 / 1024
                                );
                                break;
                            }
                            Err(error) => {
                                log::error!(
                                    "Cannot upload dropped file {}: {error}",
                                    path.display()
                                );
                            }
                        }
                    }
                    files
                })
                .await;
                if files.is_empty() {
                    return;
                }
                cx.update(|cx| {
                    dispatcher.upload_remote_paste_files(&terminal_id, files, cx);
                });
            })
            .detach();
            return;
        }

        let quoting = self.drop_path_quoting(cx);
        for path in paths.paths() {
            match quote_dropped_path(path, quoting) {
                Some(quoted) => terminal.send_input(&format!("{quoted} ")),
                None => {
                    // Controls reach the PTY as themselves: no quoting makes a name
                    // carrying LF (submits the line) or ESC (drives the emulator) safe.
                    log::warn!("Refusing to insert dropped path {path:?} into the terminal");
                    ToastManager::post(
                        Toast::warning("Dropped file skipped: its name is not safe to insert")
                            .with_detail(format!("{path:?}")),
                        cx,
                    );
                }
            }
        }
    }

    /// How the shell that reads this drop wants a path quoted. A pane shell of
    /// `Default` still means "ask the settings", the way paste resolves it, and
    /// off Windows falls back to the login shell — fish must be named, it quotes
    /// unlike every other POSIX shell.
    fn drop_path_quoting(&self, cx: &App) -> PathQuoting {
        let settings = crate::terminal_view_settings(cx);
        let ws = self.workspace.read(cx);
        let shell = self.shell_type.clone().resolve_default(
            ws.project(&self.project_id)
                .and_then(|p| p.default_shell.as_ref()),
            &settings.default_shell,
        );
        PathQuoting::for_pane_shell(&shell, &std::env::var("SHELL").unwrap_or_default())
    }
}

/// The quoting dialect of the shell a dropped path is written into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PathQuoting {
    Posix,
    /// fish reads `\\` as an escape inside single quotes, where every other
    /// POSIX shell reads it literally — the two cannot share one dialect.
    Fish,
    PowerShell,
    Cmd,
}

impl PathQuoting {
    /// Off Windows a pane that names no shell runs the login shell, which is
    /// where fish is normally met — `for_shell` alone would never see it.
    fn for_pane_shell(shell: &ShellType, login_shell: &str) -> Self {
        if *shell == ShellType::Default && !cfg!(target_os = "windows") {
            return Self::for_program(login_shell);
        }
        Self::for_shell(shell)
    }

    fn for_shell(shell: &ShellType) -> Self {
        match shell {
            #[cfg(target_os = "windows")]
            ShellType::Cmd => Self::Cmd,
            #[cfg(target_os = "windows")]
            ShellType::PowerShell { .. } => Self::PowerShell,
            #[cfg(target_os = "windows")]
            ShellType::Wsl { .. } => Self::Posix,
            ShellType::Custom { path, .. } => Self::for_program(path),
            // Unresolved `Default` is whatever the OS spawns: ComSpec, i.e. cmd.exe.
            ShellType::Default => {
                if cfg!(target_os = "windows") {
                    Self::Cmd
                } else {
                    Self::Posix
                }
            }
        }
    }

    /// A custom shell is known only by its program path.
    fn for_program(program: &str) -> Self {
        let file_name = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase();
        let stem = file_name.strip_suffix(".exe").unwrap_or(&file_name);
        match stem {
            "cmd" => Self::Cmd,
            "powershell" | "pwsh" => Self::PowerShell,
            "fish" => Self::Fish,
            _ => Self::Posix,
        }
    }

    /// Characters the shell reads literally, so a path built only from them
    /// needs no quotes at all.
    fn is_literal(self, c: char) -> bool {
        c.is_ascii_alphanumeric()
            || match self {
                PathQuoting::Posix | PathQuoting::Fish => "._-/@:+,=%".contains(c),
                PathQuoting::PowerShell | PathQuoting::Cmd => "._-/\\:".contains(c),
            }
    }
}

/// Quote a dropped path so the receiving shell reads it as one literal
/// argument, or `None` when the name cannot be written safely.
fn quote_dropped_path(path: &std::path::Path, quoting: PathQuoting) -> Option<String> {
    let path = path.to_string_lossy();
    if path.is_empty() || path.chars().any(char::is_control) {
        return None;
    }
    // cmd expands `%VAR%` and `!VAR!` inside quotes too, with no escape at the
    // prompt, and cannot escape a quote it has already opened.
    if quoting == PathQuoting::Cmd && path.contains(['"', '%', '!']) {
        return None;
    }
    if path.chars().all(|c| quoting.is_literal(c)) {
        return Some(path.into_owned());
    }
    Some(match quoting {
        PathQuoting::Posix => format!("'{}'", path.replace('\'', r"'\''")),
        PathQuoting::Fish => format!("'{}'", path.replace('\\', r"\\").replace('\'', r"\'")),
        PathQuoting::PowerShell => format!("'{}'", path.replace('\'', "''")),
        PathQuoting::Cmd => format!("\"{path}\""),
    })
}

const REMOTE_FILE_UPLOAD_LIMIT: u64 = 64 * 1024 * 1024;

fn read_remote_paste_file(path: &std::path::Path) -> Result<RemotePasteFile, String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("only files can be dropped into a remote terminal".to_string());
    }
    if metadata.len() > REMOTE_FILE_UPLOAD_LIMIT {
        return Err(format!(
            "file is larger than the {} MiB upload limit",
            REMOTE_FILE_UPLOAD_LIMIT / 1024 / 1024
        ));
    }

    let extension = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|extension| {
            !extension.is_empty()
                && extension.len() <= 16
                && extension
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric())
        })
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "bin".to_string());
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    Ok(RemotePasteFile { extension, bytes })
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
        ImageFormat::Pnm => "pnm",
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
    let ShellType::Wsl { distro } = shell else {
        return None;
    };
    if let Some(d) = distro {
        return Some(d.clone());
    }
    static DEFAULT: OnceLock<Option<String>> = OnceLock::new();
    DEFAULT
        .get_or_init(|| {
            okena_terminal::shell_config::detect_wsl_distros()
                .into_iter()
                .next()
        })
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

#[cfg(test)]
mod tests {
    use super::{PathQuoting, quote_dropped_path};
    use okena_terminal::shell_config::ShellType;
    use std::path::Path;

    const DIALECTS: [PathQuoting; 4] = [
        PathQuoting::Posix,
        PathQuoting::Fish,
        PathQuoting::PowerShell,
        PathQuoting::Cmd,
    ];

    fn quote(path: &str, quoting: PathQuoting) -> Option<String> {
        quote_dropped_path(Path::new(path), quoting)
    }

    #[test]
    fn ordinary_paths_are_inserted_unquoted() {
        assert_eq!(
            quote("/home/me/notes.md", PathQuoting::Posix).as_deref(),
            Some("/home/me/notes.md")
        );
        assert_eq!(
            quote(r"C:\Users\me\notes.md", PathQuoting::Cmd).as_deref(),
            Some(r"C:\Users\me\notes.md")
        );
    }

    #[test]
    fn posix_quoting_survives_spaces_and_single_quotes() {
        assert_eq!(
            quote("/home/me/my report's.txt", PathQuoting::Posix).as_deref(),
            Some(r"'/home/me/my report'\''s.txt'")
        );
        assert_eq!(
            quote("/home/me/$(id).txt", PathQuoting::Posix).as_deref(),
            Some("'/home/me/$(id).txt'")
        );
        assert_eq!(
            quote("/home/me/a;rm -rf ~", PathQuoting::Posix).as_deref(),
            Some("'/home/me/a;rm -rf ~'")
        );
    }

    #[test]
    fn control_characters_are_refused() {
        // A URI-decoded drop can carry any of these in the file name.
        for control in ['\n', '\r', '\t', '\x1b', '\x00', '\x7f', '\u{85}'] {
            let path = format!("/tmp/drop{control}injected");
            for quoting in DIALECTS {
                assert_eq!(
                    quote(&path, quoting),
                    None,
                    "{control:?} accepted by {quoting:?}"
                );
            }
        }
    }

    #[test]
    fn a_newline_in_a_name_cannot_reach_the_shell() {
        // `file:///tmp/a%0Arm%20-rf%20~` decodes to this.
        assert_eq!(quote("/tmp/a\nrm -rf ~", PathQuoting::Posix), None);
    }

    #[test]
    fn accepted_paths_never_carry_a_control_character() {
        let names = [
            "/tmp/plain.txt",
            "/tmp/with space.txt",
            "/tmp/it's.txt",
            "/tmp/esc\x1bhere",
            "/tmp/nl\nhere",
            r"C:\Users\me\a b.txt",
        ];
        for name in names {
            for quoting in DIALECTS {
                if let Some(quoted) = quote(name, quoting) {
                    assert!(
                        !quoted.chars().any(char::is_control),
                        "{quoting:?} emitted a control character for {name:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn windows_paths_keep_their_backslashes() {
        for quoting in [
            PathQuoting::Posix,
            PathQuoting::PowerShell,
            PathQuoting::Cmd,
        ] {
            let quoted = quote(r"C:\Users\me\my report.txt", quoting)
                .unwrap_or_else(|| panic!("{quoting:?} refused a valid Windows path"));
            assert!(
                !quoted.contains(r"\\"),
                "{quoting:?} doubled the separators: {quoted}"
            );
            assert!(quoted.contains(r"C:\Users\me\my report.txt"), "{quoted}");
        }
        // fish is the exception: inside '' it reads `\\` back as one backslash.
        assert_eq!(
            quote(r"C:\Users\me\my report.txt", PathQuoting::Fish).as_deref(),
            Some(r"'C:\\Users\\me\\my report.txt'")
        );
        assert_eq!(
            quote(r"C:\Users\me\my report.txt", PathQuoting::Cmd).as_deref(),
            Some("\"C:\\Users\\me\\my report.txt\"")
        );
        assert_eq!(
            quote(r"C:\Users\me\my report.txt", PathQuoting::Posix).as_deref(),
            Some(r"'C:\Users\me\my report.txt'")
        );
    }

    #[test]
    fn powershell_doubles_a_single_quote() {
        assert_eq!(
            quote(r"C:\Users\me\it's here.txt", PathQuoting::PowerShell).as_deref(),
            Some(r"'C:\Users\me\it''s here.txt'")
        );
    }

    #[test]
    fn cmd_refuses_a_name_it_cannot_quote() {
        // A quote cannot be escaped once cmd has opened one, and `%VAR%` /
        // `!VAR!` expand inside the quotes before the line is parsed.
        assert_eq!(quote("C:\\tmp\\a\"b.txt", PathQuoting::Cmd), None);
        assert_eq!(quote(r"C:\tmp\%PATH%.txt", PathQuoting::Cmd), None);
        assert_eq!(quote(r"C:\tmp\!PATH!.txt", PathQuoting::Cmd), None);
        // The other dialects expand neither, so they still take the name.
        assert!(quote(r"C:\tmp\%PATH%.txt", PathQuoting::PowerShell).is_some());
        assert!(quote("/tmp/100%.png", PathQuoting::Posix).is_some());
    }

    #[test]
    fn empty_paths_are_refused() {
        for quoting in DIALECTS {
            assert_eq!(quote("", quoting), None);
        }
    }

    #[test]
    fn the_dialect_follows_the_shell_program() {
        assert_eq!(PathQuoting::for_program("/bin/bash"), PathQuoting::Posix);
        assert_eq!(PathQuoting::for_program("/usr/bin/fish"), PathQuoting::Fish);
        assert_eq!(PathQuoting::for_program("/bin/dash"), PathQuoting::Posix);
        assert_eq!(
            PathQuoting::for_program(r"C:\Windows\System32\cmd.exe"),
            PathQuoting::Cmd
        );
        assert_eq!(
            PathQuoting::for_program(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            PathQuoting::PowerShell
        );
        assert_eq!(
            PathQuoting::for_program("PowerShell.exe"),
            PathQuoting::PowerShell
        );
    }

    #[test]
    fn a_custom_shell_picks_its_own_dialect() {
        assert_eq!(
            PathQuoting::for_shell(&ShellType::Custom {
                path: "/bin/zsh".to_string(),
                args: Vec::new(),
            }),
            PathQuoting::Posix
        );
        assert_eq!(
            PathQuoting::for_shell(&ShellType::Custom {
                path: "pwsh".to_string(),
                args: Vec::new(),
            }),
            PathQuoting::PowerShell
        );
        assert_eq!(
            PathQuoting::for_shell(&ShellType::Custom {
                path: "/usr/local/bin/fish".to_string(),
                args: Vec::new(),
            }),
            PathQuoting::Fish
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn an_unnamed_shell_falls_back_to_posix_off_windows() {
        assert_eq!(
            PathQuoting::for_shell(&ShellType::Default),
            PathQuoting::Posix
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn an_unnamed_shell_follows_the_login_shell_off_windows() {
        assert_eq!(
            PathQuoting::for_pane_shell(&ShellType::Default, "/usr/bin/fish"),
            PathQuoting::Fish
        );
        assert_eq!(
            PathQuoting::for_pane_shell(&ShellType::Default, "/bin/bash"),
            PathQuoting::Posix
        );
        assert_eq!(
            PathQuoting::for_pane_shell(&ShellType::Default, ""),
            PathQuoting::Posix
        );
        // A shell the pane names itself outranks the login shell.
        assert_eq!(
            PathQuoting::for_pane_shell(
                &ShellType::Custom {
                    path: "/bin/bash".to_string(),
                    args: Vec::new(),
                },
                "/usr/bin/fish"
            ),
            PathQuoting::Posix
        );
    }

    /// fish's own rules for a single-quoted word: `\\` and `\'` are the only
    /// escapes, and a bare `'` ends the word.
    fn fish_unquote(quoted: &str) -> String {
        let inner = quoted
            .strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("fish output is not a single-quoted word");
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(escaped @ ('\\' | '\'')) => out.push(escaped),
                    other => panic!("fish does not read \\{other:?} as an escape: {quoted}"),
                },
                '\'' => panic!("the word ends early, the rest is command text: {quoted}"),
                _ => out.push(c),
            }
        }
        out
    }

    #[test]
    fn fish_escapes_the_backslash_it_reads_inside_single_quotes() {
        assert_eq!(
            quote(r"/x/a\';touch /x/PWNED;\'b", PathQuoting::Fish).as_deref(),
            Some(r"'/x/a\\\';touch /x/PWNED;\\\'b'")
        );
        assert_eq!(
            quote(r"/x/y\';touch /x/PWNED3;\'", PathQuoting::Fish).as_deref(),
            Some(r"'/x/y\\\';touch /x/PWNED3;\\\''")
        );
        assert_eq!(
            quote(r"/x/trailing\", PathQuoting::Fish).as_deref(),
            Some(r"'/x/trailing\\'")
        );
        assert_eq!(
            quote(r"/x/double\\bs", PathQuoting::Fish).as_deref(),
            Some(r"'/x/double\\\\bs'")
        );
    }

    #[test]
    fn fish_round_trips_the_names_that_break_the_posix_splice() {
        for name in [
            r"/x/a\';touch /x/PWNED;\'b",
            r"/x/y\';touch /x/PWNED3;\'",
            r"/x/trailing\",
            r"/x/double\\bs",
            "/x/plain'quote",
            "/x/a b;touch /x/PWNED",
        ] {
            let quoted =
                quote(name, PathQuoting::Fish).unwrap_or_else(|| panic!("fish refused {name:?}"));
            assert_eq!(fish_unquote(&quoted), name, "round trip changed {name:?}");
        }
    }

    /// The unquoted fast path is the whole bug class, so pin both of its edges:
    /// what must never take it, and what must still take it.
    #[test]
    fn the_unquoted_fast_path_admits_exactly_the_inert_characters() {
        let unquoted = |path: &str, quoting| quote(path, quoting).as_deref() == Some(path);
        for c in [
            ' ', '\'', '"', '`', '$', '&', '|', ';', '<', '>', '(', ')', '[', ']', '{', '}', '*',
            '?', '!', '#', '~', '^',
        ] {
            let path = format!("/tmp/a{c}b");
            for quoting in DIALECTS {
                assert!(!unquoted(&path, quoting), "{quoting:?} left {c:?} unquoted");
            }
        }
        for c in ['%', ',', '@', '=', '+'] {
            let path = format!(r"C:\tmp\a{c}b");
            for quoting in [PathQuoting::PowerShell, PathQuoting::Cmd] {
                assert!(!unquoted(&path, quoting), "{quoting:?} left {c:?} unquoted");
            }
        }
        for quoting in [PathQuoting::Posix, PathQuoting::Fish] {
            assert!(
                !unquoted(r"/tmp/a\b", quoting),
                "{quoting:?} left a backslash unquoted"
            );
        }
        assert!(unquoted("/tmp/a-b_c.d/e:f+g,h=i@j%k", PathQuoting::Posix));
        assert!(unquoted("/tmp/a-b_c.d/e:f+g,h=i@j%k", PathQuoting::Fish));
        assert!(unquoted(r"C:\Users\me-1_2.txt", PathQuoting::Cmd));
        assert!(unquoted(r"C:\Users\me-1_2.txt", PathQuoting::PowerShell));
    }
}
