//! Agents view — AI coding sessions okena is running, across every project.
//!
//! There is no agent *runtime* yet: okena does not spawn, supervise or model
//! agents as entities. What it does have is the terminals those agents run in,
//! so this view reports what is actually observable — which sessions look like
//! agents, where they are, and which are blocked waiting for input — and says
//! plainly that the rest of the model (metrics, assets, MCP registration) does
//! not exist.
//!
//! Detection is a heuristic over the session's command and OSC title, so it is
//! labelled as such in the UI rather than presented as authoritative. An agent
//! that renames its own title, or one launched through a wrapper script, will
//! be missed; nothing here should be read as "these are all the agents".

use crate::theme::{theme, with_alpha};
use crate::ui::tokens::{ui_text, ui_text_ms, ui_text_sm};
use crate::workspace::state::LayoutNode;
use gpui::prelude::*;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::shell::ShellType;

use super::HarnessPane;

/// Commands that identify an AI coding agent.
///
/// Matched against the session's custom-shell command and its terminal title.
/// Deliberately a short, explicit list: a broad pattern would sweep in ordinary
/// shells and make the view untrustworthy.
pub(crate) const AGENT_COMMANDS: &[&str] = &["claude", "copilot"];

/// A session this view believes is an agent.
struct AgentRow {
    terminal_id: String,
    project_id: String,
    project_name: String,
    /// Worktree branch when the session runs in one — the sketch's
    /// "related worktree/branch".
    branch: Option<String>,
    /// Task the project is linked to, if any.
    task: Option<String>,
    kind: String,
    title: Option<String>,
    waiting: bool,
    idle: String,
    /// Whether okena's MCP server was wired into this session's launch, so the
    /// agent can actually report status and register assets.
    mcp: bool,
    /// Status the agent reported over okena's MCP server.
    reported_status: Option<String>,
    /// Assets the agent registered: (kind label, title, project).
    assets: Vec<(String, String, Option<String>)>,
}

/// Identify the agent a session is running, if any.
///
/// Returns the matched command name so the UI can label the row ("claude")
/// rather than asserting a vendor.
pub(crate) fn detect_agent(shell: &ShellType, title: Option<&str>) -> Option<String> {
    // A custom shell records the command okena launched, which is the strongest
    // signal available — it is what okena itself ran, not what the process
    // later claimed via an escape sequence.
    if let ShellType::Custom { path, .. } = shell {
        let base = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path.as_str())
            .to_ascii_lowercase();
        if let Some(cmd) = AGENT_COMMANDS.iter().find(|c| base == **c) {
            return Some((*cmd).to_string());
        }
    }
    // Fall back to the OSC title, which most agents set. Weaker: a shell
    // sitting in a directory named "claude" would match, so require the title
    // to start with the command.
    let title = title?.trim().to_ascii_lowercase();
    AGENT_COMMANDS
        .iter()
        .find(|c| title == **c || title.starts_with(&format!("{c} ")))
        .map(|c| (*c).to_string())
}

impl HarnessPane {
    fn agent_rows(&self, cx: &Context<Self>) -> Vec<AgentRow> {
        let ws = self.workspace.read(cx);
        let terminals = self.terminals.lock();
        let mut rows = Vec::new();

        for project in ws.projects() {
            let Some(layout) = project.layout.as_ref() else {
                continue;
            };
            let branch = ws
                .remote_snapshot(&project.id)
                .and_then(|s| s.git_status.as_ref())
                .and_then(|g| g.branch.clone());
            let task = project
                .task_ref
                .as_ref()
                .map(|t| format!("{} — {}", t.display_key, t.title));

            for terminal_id in layout.collect_terminal_ids() {
                // Resolve the shell the way the spawn does. A pane's own
                // `shell_type` is `Default` unless the user picked one per
                // pane; the agent command lives on the *project's*
                // `default_shell`. Reading only the node meant an agent okena
                // launched never matched, and detection silently fell back to
                // title-sniffing.
                let node_shell = layout
                    .find_terminal_path(&terminal_id)
                    .and_then(|path| layout.get_at_path(&path).cloned())
                    .and_then(|node| match node {
                        LayoutNode::Terminal { shell_type, .. } => Some(shell_type),
                        _ => None,
                    })
                    .unwrap_or_default();
                let shell = match node_shell {
                    ShellType::Default => project.default_shell.clone().unwrap_or_default(),
                    explicit => explicit,
                };

                let terminal = terminals.get(&terminal_id);
                let title = terminal.and_then(|t| t.title());
                let Some(kind) = detect_agent(&shell, title.as_deref()) else {
                    continue;
                };
                let mcp = match &shell {
                    ShellType::Custom { args, .. } => {
                        okena_app_core::workspace::actions::execute::agent_mcp::args_have_mcp(args)
                    }
                    // Detected by title alone: okena did not launch it, so it
                    // has whatever MCP config its own environment gave it.
                    _ => false,
                };

                rows.push(AgentRow {
                    terminal_id: terminal_id.clone(),
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    branch: branch.clone(),
                    task: task.clone(),
                    kind,
                    title,
                    mcp,
                    waiting: terminal.is_some_and(|t| t.is_waiting_for_input()),
                    idle: terminal
                        .map(|t| t.idle_duration_display())
                        .unwrap_or_default(),
                    reported_status: project.agent.as_ref().and_then(|a| a.status.clone()),
                    assets: project
                        .agent
                        .as_ref()
                        .map(|a| {
                            a.assets
                                .iter()
                                .map(|x| {
                                    (
                                        x.kind.label().to_string(),
                                        x.title.clone(),
                                        x.project.clone(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
        }

        // Blocked sessions first — those are the ones needing a human.
        rows.sort_by_key(|r| (!r.waiting, r.project_name.clone()));
        rows
    }

    /// Focus this agent's terminal in the terminal workspace.
    ///
    /// This is the "click through to the session" path: resolve the terminal's
    /// position in its project's layout, focus it, and leave the harness view so
    /// the terminal is actually on screen.
    fn jump_to_terminal(
        &mut self,
        project_id: String,
        terminal_id: String,
        cx: &mut Context<Self>,
    ) {
        let path = self
            .workspace
            .read(cx)
            .project(&project_id)
            .and_then(|p| p.layout.as_ref())
            .and_then(|l| l.find_terminal_path(&terminal_id));

        let Some(path) = path else {
            // The session went away between render and click.
            return;
        };

        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_focused_project_individual(fm, Some(project_id.clone()), cx);
                ws.set_focused_terminal(fm, project_id.clone(), path.clone(), cx);
            });
            cx.notify();
        });
        okena_workspace::harness_state::set_active_harness(self.window_id, None, cx);
        cx.notify();
    }

    fn render_agent_card(&self, row: &AgentRow, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let project_id = row.project_id.clone();
        let terminal_id = row.terminal_id.clone();

        let status_color = if row.waiting { t.warning } else { t.success };
        let status_label = if row.waiting {
            if row.idle.is_empty() {
                "waiting for input".to_string()
            } else {
                format!("waiting for input · {}", row.idle)
            }
        } else {
            "running".to_string()
        };

        let mut facts: Vec<AnyElement> = Vec::new();
        if let Some(branch) = &row.branch {
            facts.push(self.chip(branch.clone(), t.text_secondary, cx));
        }
        if let Some(title) = &row.title {
            facts.push(self.chip(title.clone(), t.text_muted, cx));
        }
        facts.push(if row.mcp {
            self.chip("okena mcp".to_string(), t.success, cx)
        } else {
            // Not an error: an agent started outside okena simply was not
            // handed the config.
            self.chip("no okena mcp".to_string(), t.text_muted, cx)
        });

        v_flex()
            .rounded(px(6.0))
            .border_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .px(px(10.0))
                    .py(px(7.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        h_flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                div()
                                    .text_size(ui_text(13.0, cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(format!("{} · {}", row.kind, row.project_name)),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(with_alpha(status_color, 0.15))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(status_color))
                                    .child(status_label),
                            ),
                    )
                    .child(self.small_button(
                        "agent-jump",
                        "Open session",
                        cx.listener(move |this, _, _window, cx| {
                            this.jump_to_terminal(project_id.clone(), terminal_id.clone(), cx);
                        }),
                        cx,
                    )),
            )
            .child(
                v_flex()
                    .p(px(10.0))
                    .gap(px(6.0))
                    .child(h_flex().gap(px(6.0)).flex_wrap().children(facts))
                    .children(row.task.as_ref().map(|task| {
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(format!("task: {task}"))
                            .into_any_element()
                    }))
                    // What the agent says it is doing, as opposed to what the
                    // terminal state implies.
                    .children(row.reported_status.as_ref().map(|status| {
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_primary))
                            .child(format!("“{status}”"))
                            .into_any_element()
                    }))
                    .children(row.assets.iter().map(|(kind, title, project)| {
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(match project {
                                Some(p) => format!("  · {kind}: {title} ({p})"),
                                None => format!("  · {kind}: {title}"),
                            })
                            .into_any_element()
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_agents_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let rows = self.agent_rows(cx);

        let note = div()
            .px(px(12.0))
            .py(px(6.0))
            .text_size(ui_text_ms(cx))
            .text_color(rgb(t.text_muted))
            // Say what this list is and isn't, so an empty or partial list is
            // not mistaken for "no agents are running".
            .child(
                "Sessions okena recognizes as coding agents, detected from each \
                 session's launch command and title. Agents okena starts are handed \
                 its MCP server automatically and report their own status and assets.",
            )
            .into_any_element();

        if rows.is_empty() {
            return v_flex()
                .size_full()
                .child(note)
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(14.0))
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(format!(
                            "No agent sessions detected. Looking for: {}.",
                            AGENT_COMMANDS.join(", ")
                        )),
                )
                .into_any_element();
        }

        let cards: Vec<AnyElement> = rows
            .iter()
            .map(|row| self.render_agent_card(row, cx))
            .collect();

        v_flex()
            .size_full()
            .child(note)
            .child(
                v_flex()
                    .id("agents-view-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .px(px(12.0))
                    .pb(px(12.0))
                    .gap(px(10.0))
                    .children(cards),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: that pulls in the `gpui::*` glob, whose `test`
    // attribute macro shadows the built-in one and recurses forever.
    use super::{AGENT_COMMANDS, detect_agent};
    use okena_core::shell::ShellType;

    fn custom(path: &str) -> ShellType {
        ShellType::Custom {
            path: path.to_string(),
            args: Vec::new(),
        }
    }

    #[test]
    fn detects_agent_from_launch_command() {
        assert_eq!(
            detect_agent(&custom("claude"), None).as_deref(),
            Some("claude")
        );
        // An absolute path still resolves to its basename.
        assert_eq!(
            detect_agent(&custom("/opt/homebrew/bin/copilot"), None).as_deref(),
            Some("copilot")
        );
    }

    #[test]
    fn plain_shell_is_not_an_agent() {
        assert_eq!(detect_agent(&ShellType::Default, None), None);
        assert_eq!(detect_agent(&custom("/bin/zsh"), None), None);
    }

    #[test]
    fn detects_agent_from_title() {
        assert_eq!(
            detect_agent(&ShellType::Default, Some("claude")).as_deref(),
            Some("claude")
        );
        assert_eq!(
            detect_agent(&ShellType::Default, Some("Copilot working…")).as_deref(),
            Some("copilot")
        );
    }

    #[test]
    fn title_must_start_with_the_command() {
        // A shell sitting in a directory named after an agent must not match —
        // that would fill the view with things that aren't agents.
        assert_eq!(
            detect_agent(&ShellType::Default, Some("~/src/claude")),
            None
        );
        assert_eq!(
            detect_agent(&ShellType::Default, Some("vim claude.rs")),
            None
        );
    }

    #[test]
    fn a_pane_inheriting_the_projects_shell_is_detected() {
        // What okena actually produces: the pane is `Default` and the agent
        // command sits on the project. Resolving only the pane finds nothing.
        let node = ShellType::Default;
        let project_default = Some(custom("claude"));
        let resolved = match node {
            ShellType::Default => project_default.clone().unwrap_or_default(),
            explicit => explicit,
        };
        assert_eq!(detect_agent(&resolved, None).as_deref(), Some("claude"));
    }

    #[test]
    fn an_explicit_pane_shell_wins_over_the_projects() {
        // A pane the user pointed at a plain shell must not be reported as an
        // agent just because the project defaults to one.
        let node = custom("/bin/zsh");
        let project_default = Some(custom("claude"));
        let resolved = match node {
            ShellType::Default => project_default.clone().unwrap_or_default(),
            explicit => explicit,
        };
        assert_eq!(detect_agent(&resolved, None), None);
    }

    #[test]
    fn command_match_is_exact_not_substring() {
        // `claudius` is not `claude`.
        assert_eq!(detect_agent(&custom("claudius"), None), None);
    }

    #[test]
    fn agent_command_list_is_lowercase() {
        // Detection lowercases its input, so an uppercase entry could never match.
        for c in AGENT_COMMANDS {
            assert_eq!(*c, &c.to_ascii_lowercase(), "{c} must be lowercase");
        }
    }
}
