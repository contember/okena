use super::{WindowView, WindowViewEvent};
use gpui::{prelude::*, *};
use okena_core::{
    api::ActionRequest,
    attention::{AttentionEpisode, AttentionKind},
    mission::*,
};
use okena_transport::client::{ConnectionStatus, make_prefixed_id, strip_prefix};
use okena_ui::{
    simple_input::{SimpleInput, SimpleInputState},
    theme::theme,
};
use okena_workspace::state::{MissionSelection, WorkNavigation};

pub(super) struct MissionDraft {
    connection_id: String,
    mission_id: Option<String>,
    title: Entity<SimpleInputState>,
    goal: Entity<SimpleInputState>,
    home: Option<String>,
    member: Option<MissionMember>,
    error: Option<String>,
    submission: Option<uuid::Uuid>,
}

#[derive(serde::Deserialize)]
struct MissionSaved {
    mission_id: String,
}

impl MissionSaved {
    fn parse(value: serde_json::Value) -> Result<Self, String> {
        let saved: Self = serde_json::from_value(value)
            .map_err(|error| format!("Invalid mission response: {error}"))?;
        if saved.mission_id.is_empty() {
            return Err("The daemon returned an empty mission ID.".into());
        }
        Ok(saved)
    }
}

impl MissionDraft {
    fn finish_submission(
        &mut self,
        submission: uuid::Uuid,
        result: Result<MissionSaved, String>,
    ) -> Option<MissionSelection> {
        if self.submission != Some(submission) {
            return None;
        }
        self.submission = None;
        match result {
            Ok(saved) => Some(MissionSelection {
                connection_id: self.connection_id.clone(),
                mission_id: saved.mission_id,
            }),
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}

#[derive(Clone)]
struct Owner {
    id: String,
    name: String,
    connected: bool,
    overview: Option<WorkOverview>,
}

fn route_work_action(
    owners: &[Owner],
    connection_id: &str,
    action: ActionRequest,
    dispatch: impl FnOnce(&str, ActionRequest),
) -> bool {
    let Some(owner) = owners
        .iter()
        .find(|owner| owner.id == connection_id && owner.connected && owner.overview.is_some())
    else {
        return false;
    };
    dispatch(&owner.id, action);
    true
}

fn mission_worktree_target_available(
    data: &okena_workspace::state::WorkspaceData,
    owners: &[Owner],
    selection: &MissionSelection,
    project_id: &str,
) -> bool {
    data.projects.iter().any(|project| {
        project.id == project_id
            && project.connection_id.as_deref() == Some(selection.connection_id.as_str())
            && project.worktree_info.is_none()
    }) && owners.iter().any(|owner| {
        owner.id == selection.connection_id
            && owner.connected
            && owner.overview.as_ref().is_some_and(|overview| {
                overview
                    .missions
                    .iter()
                    .any(|mission| mission.id == selection.mission_id)
            })
    })
}

fn worktree_create_request(
    data: &okena_workspace::state::WorkspaceData,
    owners: &[Owner],
    mission: Option<&MissionSelection>,
    project_id: &str,
    branch: &str,
    create_branch: bool,
) -> Option<ActionRequest> {
    if mission.is_some_and(|mission| {
        !mission_worktree_target_available(data, owners, mission, project_id)
    }) {
        return None;
    }
    Some(ActionRequest::CreateWorktree {
        project_id: project_id.into(),
        branch: branch.into(),
        create_branch,
        mission_id: mission.map(|mission| mission.mission_id.clone()),
    })
}

fn sorted_attention(
    owners: impl IntoIterator<Item = (String, Vec<AttentionEpisode>)>,
) -> Vec<(String, AttentionEpisode)> {
    let mut items: Vec<_> = owners
        .into_iter()
        .flat_map(|(id, episodes)| {
            episodes
                .into_iter()
                .filter(|e| !e.read)
                .map(move |e| (id.clone(), e))
        })
        .collect();
    items.sort_by(|(ac, a), (bc, b)| {
        (a.kind == AttentionKind::Completion, a.created_at, ac, &a.id).cmp(&(
            b.kind == AttentionKind::Completion,
            b.created_at,
            bc,
            &b.id,
        ))
    });
    items
}

fn button(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id.into())
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .child(label.into())
        .hover(|s| s.opacity(0.75))
}

impl WindowView {
    pub(super) fn worktree_create_request(
        &self,
        mission: Option<&MissionSelection>,
        project_id: &str,
        branch: &str,
        create_branch: bool,
        cx: &App,
    ) -> Option<ActionRequest> {
        worktree_create_request(
            self.workspace.read(cx).data(),
            &self.work_owners(cx),
            mission,
            project_id,
            branch,
            create_branch,
        )
    }
    pub(super) fn mission_worktree_target_available(
        &self,
        selection: &MissionSelection,
        project_id: &str,
        cx: &App,
    ) -> bool {
        mission_worktree_target_available(
            self.workspace.read(cx).data(),
            &self.work_owners(cx),
            selection,
            project_id,
        )
    }

    fn open_mission_worktree(
        &mut self,
        project_id: String,
        mission: okena_views_git::worktree_dialog::MissionWorktreeContext,
        cx: &mut Context<Self>,
    ) {
        if !self.mission_worktree_target_available(&mission.selection, &project_id, cx) {
            return;
        }
        if let Some(params) = self.remote_params(&project_id, &mission.selection.connection_id, cx)
        {
            self.overlay_manager.update(cx, |manager, cx| {
                manager.show_worktree_dialog(project_id, params, Some(mission), cx)
            });
        }
    }

    pub(super) fn open_work_source(
        &mut self,
        project_id: &str,
        terminal_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.workspace.read(cx).project(project_id) else {
            return;
        };
        let Some(connection) = project.connection_id.clone() else {
            return;
        };
        let raw_project = strip_prefix(project_id, &connection);
        let member = if let Some(terminal) = terminal_id {
            let raw_terminal = strip_prefix(terminal, &connection);
            let conversation = self
                .workspace
                .read(cx)
                .data()
                .remote_work_overviews
                .get(&connection)
                .and_then(|o| {
                    o.conversations
                        .iter()
                        .find(|a| a.terminal_id == raw_terminal)
                })
                .map(|a| a.conversation.clone());
            conversation
                .map(|conversation| MissionMember::Conversation { conversation })
                .unwrap_or(MissionMember::Terminal {
                    project_id: raw_project,
                    terminal_id: raw_terminal,
                })
        } else if project.worktree_info.is_some() {
            MissionMember::Worktree {
                project_id: raw_project,
            }
        } else {
            MissionMember::Repository {
                project_id: raw_project,
            }
        };
        self.navigate_work(WorkNavigation::Missions, cx);
        self.work_source = Some((connection, member));
        cx.notify();
    }
    fn work_owners(&self, cx: &App) -> Vec<Owner> {
        let Some(manager) = &self.remote_manager else {
            return vec![];
        };
        let mut owners: Vec<_> = manager
            .read(cx)
            .connections()
            .into_iter()
            .map(|(config, status, state)| Owner {
                id: config.id.clone(),
                name: config.name.clone(),
                connected: matches!(status, ConnectionStatus::Connected) && state.is_some(),
                overview: self
                    .workspace
                    .read(cx)
                    .data()
                    .remote_work_overviews
                    .get(&config.id)
                    .cloned(),
            })
            .collect();
        owners.sort_by(|a, b| a.id.cmp(&b.id));
        owners
    }

    fn work_navigation(&self, cx: &App) -> WorkNavigation {
        self.workspace
            .read(cx)
            .data()
            .window(self.window_id)
            .map(|w| w.work_navigation)
            .unwrap_or_default()
    }

    fn navigate_work(&mut self, navigation: WorkNavigation, cx: &mut Context<Self>) {
        self.close_work_draft(cx);
        self.workspace.update(cx, |ws, cx| {
            if let Some(state) = ws.data.window_mut(self.window_id) {
                state.work_navigation = navigation;
            }
            ws.notify_data(cx);
        });
        cx.notify();
    }

    fn select_mission(&mut self, selection: MissionSelection, cx: &mut Context<Self>) {
        self.workspace.update(cx, |ws, cx| {
            if let Some(state) = ws.data.window_mut(self.window_id) {
                state.work_navigation = WorkNavigation::Missions;
                state.selected_mission = Some(selection);
            }
            ws.notify_data(cx);
        });
        cx.notify();
    }

    fn send_work(&self, connection_id: &str, action: ActionRequest, cx: &mut Context<Self>) {
        route_work_action(
            &self.work_owners(cx),
            connection_id,
            action,
            |connection_id, action| {
                if let Some(dispatcher) = crate::action_dispatch::dispatcher_for_connection(
                    connection_id,
                    self.window_id,
                    &self.workspace,
                    &self.focus_manager,
                    &self.remote_manager,
                ) {
                    dispatcher.dispatch(action, cx);
                }
            },
        );
    }

    fn mission_command(
        &self,
        connection_id: &str,
        command: MissionCommand,
        cx: &mut Context<Self>,
    ) {
        self.send_work(connection_id, ActionRequest::Mission { command }, cx);
    }

    fn reveal_work(
        &mut self,
        connection_id: String,
        project_id: String,
        terminal_id: String,
        completion: Option<AttentionEpisode>,
        cx: &mut Context<Self>,
    ) {
        self.close_work_draft(cx);
        cx.emit(WindowViewEvent::RevealWork {
            origin: self.window_id,
            connection_id,
            project_id,
            terminal_id,
            completion: completion.map(Box::new),
        });
    }

    pub(super) fn render_work_navigation(&self, cx: &mut Context<Self>) -> AnyElement {
        let navigation = self.work_navigation(cx);
        let count: usize = self
            .work_owners(cx)
            .iter()
            .filter_map(|o| o.overview.as_ref())
            .map(|o| o.attention.iter().filter(|e| !e.read).count())
            .sum();
        let t = theme(cx);
        div()
            .flex()
            .gap_2()
            .px_2()
            .py_1()
            .bg(rgb(t.bg_header))
            .border_b_1()
            .border_color(rgb(t.border))
            .children(
                [
                    (WorkNavigation::Projects, "Projects".to_string()),
                    (WorkNavigation::Inbox, format!("Inbox · {count}")),
                    (WorkNavigation::Missions, "Missions".to_string()),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, (target, label))| {
                    button(format!("work-nav-{i}"), label)
                        .when(navigation == target, |d| {
                            d.bg(rgb(t.bg_secondary)).text_color(rgb(t.border_active))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.navigate_work(target, cx)))
                }),
            )
            .into_any_element()
    }

    pub(super) fn render_work_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let navigation = self.work_navigation(cx);
        if navigation == WorkNavigation::Projects {
            return None;
        }
        let owners = self.work_owners(cx);
        let t = theme(cx);
        let mut content = div()
            .id("work-panel")
            .w(px(320.0))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(t.bg_secondary))
            .border_r_1()
            .border_color(rgb(t.border))
            .text_color(rgb(t.text_primary))
            .text_size(okena_ui::tokens::ui_text_md(cx));
        if owners.is_empty() {
            content = content.child("Connect a daemon to see your work.");
        }
        for owner in &owners {
            if !owner.connected || owner.overview.is_none() {
                content = content.child(div().text_color(rgb(t.text_muted)).child(format!(
                    "{} — {}",
                    owner.name,
                    if !owner.connected {
                        "Disconnected · last known work"
                    } else {
                        "Inbox and missions unsupported by this daemon"
                    }
                )));
            }
            if let Some(overview) = &owner.overview
                && overview.lost_transitions > 0
            {
                content = content.child(format!(
                    "{} · {} status updates unavailable",
                    owner.name, overview.lost_transitions
                ));
            }
        }
        if navigation == WorkNavigation::Inbox {
            let items = sorted_attention(owners.iter().filter_map(|o| {
                o.overview
                    .as_ref()
                    .map(|v| (o.id.clone(), v.attention.clone()))
            }));
            if items.is_empty() {
                content = content.child("Nothing needs your attention.");
            }
            let mut previous = None;
            for (index, (connection_id, episode)) in items.into_iter().enumerate() {
                if previous != Some(episode.kind) {
                    content = content.child(div().font_weight(FontWeight::SEMIBOLD).mt_2().child(
                        if episode.kind == AttentionKind::InputNeeded {
                            "Input needed · oldest first"
                        } else {
                            "Unread completions · oldest first"
                        },
                    ));
                    previous = Some(episode.kind);
                }
                let owner = owners.iter().find(|o| o.id == connection_id);
                let connected = owner.is_some_and(|o| o.connected);
                let project_id = make_prefixed_id(&connection_id, &episode.source.project_id);
                let terminal_id = make_prefixed_id(&connection_id, &episode.source.terminal_id);
                let project = self.workspace.read(cx).project(&project_id);
                let name = project
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Unavailable project".into());
                let live = connected
                    && episode.available
                    && project
                        .and_then(|p| p.layout.as_ref())
                        .is_some_and(|l| l.find_terminal_path(&terminal_id).is_some());
                let reveal_episode = episode.clone();
                let reveal_connection = connection_id.clone();
                let mut card = div()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(!connected, |d| d.opacity(0.5))
                    .child(episode.summary.clone())
                    .child(div().text_color(rgb(t.text_muted)).child(format!(
                        "{} · {}",
                        owner.map(|o| o.name.as_str()).unwrap_or("Daemon"),
                        name
                    )));
                if live {
                    card = card.child(
                        button(format!("inbox-open-{index}"), "Open terminal").on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.reveal_work(
                                    reveal_connection.clone(),
                                    reveal_episode.source.project_id.clone(),
                                    reveal_episode.source.terminal_id.clone(),
                                    (reveal_episode.kind == AttentionKind::Completion)
                                        .then(|| reveal_episode.clone()),
                                    cx,
                                );
                            }),
                        ),
                    );
                    card = card.child(div().text_color(rgb(t.text_muted)).child(format!(
                            "{} · {}",
                            episode
                                .source
                                .conversation
                                .as_ref()
                                .map(|c| c.agent.as_str())
                                .unwrap_or("Terminal"),
                            i64::try_from(episode.created_at)
                                .map(okena_git::format_relative_time)
                                .unwrap_or_else(|_| "unknown age".into())
                        )));
                } else {
                    card = card.child(
                        div()
                            .text_color(rgb(t.text_muted))
                            .child("Live status unconfirmed / terminal unavailable"),
                    );
                }
                if connected && (episode.kind == AttentionKind::Completion || !episode.available) {
                    card = card.child(
                        button(
                            format!("inbox-read-{index}"),
                            if episode.kind == AttentionKind::Completion {
                                "Mark read"
                            } else {
                                "Dismiss unavailable observation"
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send_work(
                                &connection_id,
                                ActionRequest::AcknowledgeAttention {
                                    episode_id: episode.id.clone(),
                                    revision: episode.revision,
                                    dismiss: episode.kind == AttentionKind::InputNeeded,
                                },
                                cx,
                            );
                        })),
                    );
                }
                content = content.child(card);
            }
        } else {
            content = content.child(self.render_missions(&owners, cx));
        }
        Some(content.into_any_element())
    }

    pub(super) fn close_work_draft(&mut self, cx: &mut Context<Self>) {
        if self.work_draft.take().is_some() {
            self.focus_manager.update(cx, |fm, cx| {
                fm.exit_modal();
                cx.notify();
            });
        }
    }

    fn edit_mission(
        &mut self,
        connection_id: String,
        mission: Option<Mission>,
        member: Option<MissionMember>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_work_draft(cx);
        let title = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Mission title")
                .default_value(
                    mission
                        .as_ref()
                        .map(|m| m.title.clone())
                        .unwrap_or_default(),
                )
        });
        let goal = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Optional goal")
                .multiline()
                .default_value(
                    mission
                        .as_ref()
                        .and_then(|m| m.goal.clone())
                        .unwrap_or_default(),
                )
        });
        self.work_draft = Some(MissionDraft {
            connection_id,
            mission_id: mission.as_ref().map(|m| m.id.clone()),
            title: title.clone(),
            goal,
            home: mission.and_then(|m| m.home_project_id),
            member,
            error: None,
            submission: None,
        });
        self.focus_manager.update(cx, |fm, cx| {
            fm.enter_modal();
            cx.notify();
        });
        window.focus(&title.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn save_mission(&mut self, cx: &mut Context<Self>) {
        let owners = self.work_owners(cx);
        let Some(draft) = self.work_draft.as_mut() else {
            return;
        };
        if draft.submission.is_some() {
            return;
        }
        if !owners.iter().any(|owner| {
            owner.id == draft.connection_id && owner.connected && owner.overview.is_some()
        }) {
            draft.error = Some("The owning daemon is unavailable. Reconnect before saving.".into());
            cx.notify();
            return;
        }
        let title = draft.title.read(cx).value().trim().to_string();
        let goal = draft.goal.read(cx).value().trim().to_string();
        let goal = (!goal.is_empty()).then_some(goal);
        if let Err(error) = validate_text(&title, goal.as_deref()) {
            draft.error = Some(error);
            cx.notify();
            return;
        }
        let connection_id = draft.connection_id.clone();
        let creating = draft.mission_id.is_none();
        if creating
            && let Some(member) = &draft.member
            && let Some(existing) = owners
                .iter()
                .find(|owner| owner.id == connection_id)
                .and_then(|owner| owner.overview.as_ref())
                .and_then(|overview| member_owner(overview, member))
        {
            draft.error = Some(format!(
                "This work belongs to ‘{}’. Create without this member, or cancel and use the explicit Move action.",
                existing.title
            ));
            cx.notify();
            return;
        }
        let command = if let Some(id) = &draft.mission_id {
            MissionCommand::Edit {
                mission_id: id.clone(),
                title,
                goal,
                home_project_id: draft.home.clone(),
            }
        } else {
            MissionCommand::Create {
                title,
                goal,
                home_project_id: draft.home.clone(),
                member: draft.member.clone(),
            }
        };
        let Some(manager) = self.remote_manager.clone() else {
            draft.error = Some("The owning daemon is unavailable.".into());
            cx.notify();
            return;
        };
        let submission = uuid::Uuid::new_v4();
        draft.submission = Some(submission);
        draft.error = None;
        let response = manager.update(cx, |manager, cx| {
            manager.send_action_with_result(&connection_id, ActionRequest::Mission { command }, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = response.await.and_then(MissionSaved::parse);
            let _ = this.update(cx, |this, cx| {
                let Some(draft) = this.work_draft.as_mut() else {
                    return;
                };
                if let Some(selection) = draft.finish_submission(submission, result) {
                    if creating {
                        this.mission_filter = MissionLifecycle::Active;
                    }
                    this.select_mission(selection, cx);
                    this.close_work_draft(cx);
                    this.work_source = None;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_missions(&mut self, owners: &[Owner], cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap_2();
        if self.work_source.as_ref().is_some_and(|(owner, member)| {
            let workspace = self.workspace.read(cx);
            match member {
                MissionMember::Repository { project_id }
                | MissionMember::Worktree { project_id } => workspace
                    .project(&make_prefixed_id(owner, project_id))
                    .is_none(),
                MissionMember::Terminal {
                    project_id,
                    terminal_id,
                } => workspace
                    .project(&make_prefixed_id(owner, project_id))
                    .and_then(|p| p.layout.as_ref())
                    .is_none_or(|layout| {
                        layout
                            .find_terminal_path(&make_prefixed_id(owner, terminal_id))
                            .is_none()
                    }),
                MissionMember::Conversation { .. } => false,
            }
        }) {
            self.work_source = None;
            body = body.child("The selected project or terminal is no longer available.");
        }
        if self.work_source.is_some() {
            body = body
                .child("Create a mission from the selected work, or select a mission to attach it.")
                .child(
                    button("clear-work-source", "Cancel attachment").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.work_source = None;
                            cx.notify();
                        },
                    )),
                );
        }
        if self.work_draft.is_some() {
            return self.render_mission_editor(cx);
        }
        body = body.child(
            div().flex().gap_1().children(
                [
                    (MissionLifecycle::Active, "Active"),
                    (MissionLifecycle::Done, "Done"),
                    (MissionLifecycle::Archived, "Archived"),
                ]
                .into_iter()
                .map(|(filter, label)| {
                    button(format!("mission-filter-{label}"), label)
                        .when(self.mission_filter == filter, |d| {
                            d.font_weight(FontWeight::BOLD)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.mission_filter = filter;
                            cx.notify();
                        }))
                }),
            ),
        );
        let selection = self
            .workspace
            .read(cx)
            .data()
            .window(self.window_id)
            .and_then(|w| w.selected_mission.clone());
        for owner in owners {
            let Some(overview) = &owner.overview else {
                continue;
            };
            let connection = owner.id.clone();
            if owner.connected
                && self
                    .work_source
                    .as_ref()
                    .is_none_or(|(source, _)| source == &owner.id)
            {
                body = body.child(
                    button(
                        format!("new-mission-{}", owner.id),
                        format!("+ New mission · {}", owner.name),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let member = this
                            .work_source
                            .as_ref()
                            .filter(|(owner, _)| owner == &connection)
                            .map(|(_, member)| member.clone());
                        this.edit_mission(connection.clone(), None, member, window, cx);
                    })),
                );
            }
            let missions: Vec<_> = overview
                .missions
                .iter()
                .filter(|m| m.lifecycle == self.mission_filter)
                .collect();
            for mission in missions {
                let selection = MissionSelection {
                    connection_id: owner.id.clone(),
                    mission_id: mission.id.clone(),
                };
                let count = overview
                    .attention
                    .iter()
                    .filter(|e| !e.read && mission_has_source(mission, overview, e))
                    .count();
                body =
                    body.child(
                        button(
                            format!("mission-{}-{}", owner.id, mission.id),
                            format!("{} · {} · {count}", mission.title, owner.name),
                        )
                        .when(!owner.connected, |d| d.opacity(0.5))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_mission(selection.clone(), cx)
                        })),
                    );
            }
        }
        if let Some(selection) = selection {
            if let Some(owner) = owners.iter().find(|o| o.id == selection.connection_id)
                && let Some(overview) = &owner.overview
                && let Some(mission) = overview
                    .missions
                    .iter()
                    .find(|m| m.id == selection.mission_id)
            {
                body = body.child(self.render_mission_board(owner, overview, mission, cx));
            } else {
                body = body.child("Selected mission is unavailable. Select another mission above.");
            }
        } else {
            body = body.child("Select a mission, or create one from existing work.");
        }
        body.into_any_element()
    }

    fn render_mission_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(draft) = &self.work_draft else {
            return div().into_any_element();
        };
        if draft.submission.is_some() {
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .child(draft.title.read(cx).value().to_string())
                .child(draft.goal.read(cx).value().to_string())
                .child("Saving…")
                .child(
                    button("mission-cancel-pending", "Close editor").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.close_work_draft(cx);
                            cx.notify();
                        },
                    )),
                )
                .into_any_element();
        }
        let t = theme(cx);
        let mut form = div()
            .flex()
            .flex_col()
            .gap_2()
            .child("Title")
            .child(okena_ui::input::input_container(&t, None).child(SimpleInput::new(&draft.title)))
            .child("Goal")
            .child(okena_ui::input::input_container(&t, None).child(SimpleInput::new(&draft.goal)))
            .child("Home project (optional)");
        form = form.child(
            button(
                "mission-home-none",
                if draft.home.is_none() {
                    "✓ No home project"
                } else {
                    "No home project"
                },
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(d) = &mut this.work_draft {
                    d.home = None;
                }
                cx.notify();
            })),
        );
        for project in self
            .workspace
            .read(cx)
            .projects()
            .iter()
            .filter(|p| p.connection_id.as_deref() == Some(&draft.connection_id))
        {
            let id = strip_prefix(&project.id, &draft.connection_id);
            form = form.child(
                button(
                    format!("mission-home-{}", project.id),
                    format!(
                        "{}{}",
                        if draft.home.as_ref() == Some(&id) {
                            "✓ "
                        } else {
                            ""
                        },
                        project.name
                    ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(d) = &mut this.work_draft {
                        d.home = Some(id.clone());
                    }
                    cx.notify();
                })),
            );
        }
        if let Some(error) = &draft.error {
            form = form.child(error.clone());
        }
        if draft.mission_id.is_none() && draft.member.is_some() {
            form = form.child(
                button(
                    "mission-create-without-member",
                    "Create without this member",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(draft) = &mut this.work_draft {
                        draft.member = None;
                        draft.error = None;
                    }
                    cx.notify();
                })),
            );
        }
        form.child(
            div()
                .flex()
                .gap_2()
                .child(
                    button("mission-save", "Save")
                        .on_click(cx.listener(|this, _, _, cx| this.save_mission(cx))),
                )
                .child(button("mission-cancel", "Cancel").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.close_work_draft(cx);
                        cx.notify();
                    },
                ))),
        )
        .into_any_element()
    }

    fn render_mission_board(
        &self,
        owner: &Owner,
        overview: &WorkOverview,
        mission: &Mission,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let mut board = div()
            .when(!owner.connected, |d| d.opacity(0.5))
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(rgb(t.border))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(mission.title.clone()),
            )
            .child(format!("{} · {:?}", owner.name, mission.lifecycle));
        if let Some(goal) = &mission.goal {
            board = board.child(goal.clone());
        }
        if let Some(home) = &mission.home_project_id {
            board = board.child(format!(
                "Home: {}",
                self.workspace
                    .read(cx)
                    .project(&make_prefixed_id(&owner.id, home))
                    .map(|p| p.name.as_str())
                    .unwrap_or("Unavailable project")
            ));
        }
        if owner.connected {
            let connection = owner.id.clone();
            let edit = mission.clone();
            board = board.child(
                button("mission-edit", "Edit title, goal and home").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.edit_mission(connection.clone(), Some(edit.clone()), None, window, cx)
                    },
                )),
            );
            for (lifecycle, label) in match mission.lifecycle {
                MissionLifecycle::Active => vec![
                    (MissionLifecycle::Done, "Mark done"),
                    (MissionLifecycle::Archived, "Archive"),
                ],
                MissionLifecycle::Done => vec![
                    (MissionLifecycle::Active, "Reopen"),
                    (MissionLifecycle::Archived, "Archive"),
                ],
                MissionLifecycle::Archived => vec![(MissionLifecycle::Active, "Restore")],
            } {
                let connection = owner.id.clone();
                let id = mission.id.clone();
                board = board.child(
                    button(format!("mission-lifecycle-{label}"), label).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.mission_command(
                                &connection,
                                MissionCommand::SetLifecycle {
                                    mission_id: id.clone(),
                                    lifecycle,
                                },
                                cx,
                            )
                        },
                    )),
                );
            }
        }
        for episode in overview
            .attention
            .iter()
            .filter(|e| !e.read && mission_has_source(mission, overview, e))
        {
            board = board.child(format!(
                "{} · {}{}",
                if episode.kind == AttentionKind::InputNeeded {
                    "Input needed"
                } else {
                    "Unread completion"
                },
                episode.summary,
                if episode.available && owner.connected {
                    ""
                } else {
                    " · unconfirmed"
                }
            ));
        }
        board = board.child(div().font_weight(FontWeight::SEMIBOLD).child("Members"));
        if let Some((connection, member)) = &self.work_source {
            if connection == &owner.id {
                board = board.child(self.render_member(
                    owner,
                    mission,
                    member.clone(),
                    usize::MAX,
                    false,
                    cx,
                ));
            } else {
                board = board.child(
                    "Selected work belongs to another daemon. Choose a mission on that daemon.",
                );
            }
        }
        let members = mission_members(mission, overview);
        if members.is_empty() {
            board = board.child("No members yet. Attach existing work below.");
        }
        for (index, member) in members.into_iter().enumerate() {
            board = board.child(self.render_member(owner, mission, member, index, true, cx));
        }
        for pr in &mission.pull_requests {
            let url = pr.info.url.clone();
            board = board.child(
                button(
                    format!(
                        "pr-{}-{}-{}",
                        pr.identity.host, pr.identity.repository, pr.identity.number
                    ),
                    format!(
                        "{}/{} #{} · {}{}",
                        pr.identity.host,
                        pr.identity.repository,
                        pr.identity.number,
                        pr.info.state.label(),
                        if pr.available && owner.connected {
                            ""
                        } else {
                            " · last known"
                        }
                    ),
                )
                .on_click(move |_, _, cx| cx.open_url(&url)),
            );
            board = board.child(format!(
                "Observed: {} · CI: {}",
                i64::try_from(pr.observed_at)
                    .map(okena_git::format_relative_time)
                    .unwrap_or_else(|_| "unknown".into()),
                pr.ci
                    .as_ref()
                    .map(|ci| ci.tooltip_text())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
        if owner.connected {
            board = board.child(
                div()
                    .mt_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Create worktree for this mission"),
            );
            for project in self.workspace.read(cx).projects().iter().filter(|project| {
                project.connection_id.as_deref() == Some(owner.id.as_str())
                    && project.worktree_info.is_none()
            }) {
                let project_id = project.id.clone();
                let context = okena_views_git::worktree_dialog::MissionWorktreeContext {
                    selection: MissionSelection {
                        connection_id: owner.id.clone(),
                        mission_id: mission.id.clone(),
                    },
                    title: mission.title.clone(),
                };
                board = board.child(
                    button(
                        format!("mission-create-worktree-{}", project.id),
                        format!("Create worktree in {}…", project.name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_mission_worktree(project_id.clone(), context.clone(), cx)
                    })),
                );
            }
            board = board.child(
                div()
                    .mt_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Attach existing work"),
            );
            let mut candidates = Vec::new();
            for project in self
                .workspace
                .read(cx)
                .projects()
                .iter()
                .filter(|p| p.connection_id.as_deref() == Some(&owner.id))
            {
                let id = strip_prefix(&project.id, &owner.id);
                candidates.push(if project.worktree_info.is_some() {
                    MissionMember::Worktree {
                        project_id: id.clone(),
                    }
                } else {
                    MissionMember::Repository {
                        project_id: id.clone(),
                    }
                });
                if let Some(layout) = &project.layout {
                    for terminal in layout.collect_terminal_ids() {
                        candidates.push(MissionMember::Terminal {
                            project_id: id.clone(),
                            terminal_id: strip_prefix(&terminal, &owner.id),
                        });
                    }
                }
            }
            let mut conversations: Vec<_> = overview
                .conversations
                .iter()
                .map(|a| a.conversation.clone())
                .chain(
                    overview
                        .missions
                        .iter()
                        .flat_map(|m| m.conversations.iter().cloned()),
                )
                .collect();
            conversations.sort_by(|a, b| (&a.agent, &a.session_id).cmp(&(&b.agent, &b.session_id)));
            conversations.dedup();
            candidates.extend(
                conversations
                    .into_iter()
                    .map(|conversation| MissionMember::Conversation { conversation }),
            );
            for (index, member) in candidates.into_iter().enumerate() {
                if !mission_members(mission, overview).contains(&member) {
                    board =
                        board.child(self.render_member(owner, mission, member, index, false, cx));
                }
            }
        }
        board.into_any_element()
    }

    fn render_member(
        &self,
        owner: &Owner,
        mission: &Mission,
        member: MissionMember,
        index: usize,
        attached: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(overview) = &owner.overview else {
            return div().into_any_element();
        };
        let (label, project, terminal) = match &member {
            MissionMember::Repository { project_id } | MissionMember::Worktree { project_id } => {
                let p = self
                    .workspace
                    .read(cx)
                    .project(&make_prefixed_id(&owner.id, project_id));
                let terminal = p
                    .and_then(|p| p.layout.as_ref())
                    .and_then(|l| l.collect_terminal_ids().first().cloned())
                    .map(|t| strip_prefix(&t, &owner.id));
                (
                    format!(
                        "{} · {}",
                        if matches!(member, MissionMember::Worktree { .. }) {
                            "Worktree"
                        } else {
                            "Repository"
                        },
                        p.map(|p| p.name.as_str()).unwrap_or("Unavailable")
                    ),
                    Some(project_id.clone()),
                    terminal,
                )
            }
            MissionMember::Terminal {
                project_id,
                terminal_id,
            } => {
                let p = self
                    .workspace
                    .read(cx)
                    .project(&make_prefixed_id(&owner.id, project_id));
                let tid = make_prefixed_id(&owner.id, terminal_id);
                (
                    p.map(|p| format!("{} · {}", p.name, p.terminal_display_name(&tid, None)))
                        .unwrap_or_else(|| "Unavailable terminal".into()),
                    Some(project_id.clone()),
                    Some(terminal_id.clone()),
                )
            }
            MissionMember::Conversation { conversation } => {
                let attachment = overview
                    .conversations
                    .iter()
                    .filter(|a| a.conversation == *conversation)
                    .min_by(|a, b| {
                        (&a.project_id, &a.terminal_id).cmp(&(&b.project_id, &b.terminal_id))
                    });
                (
                    format!(
                        "{} · {}{}",
                        conversation.agent,
                        conversation.session_id,
                        if attachment.is_none() {
                            " · offline history"
                        } else {
                            ""
                        }
                    ),
                    attachment.map(|a| a.project_id.clone()),
                    attachment.map(|a| a.terminal_id.clone()),
                )
            }
        };
        let mut row = div().py_1().flex().flex_col().gap_1().child(label);
        if matches!(
            member,
            MissionMember::Repository { .. } | MissionMember::Worktree { .. }
        ) && let Some(project) = &project
            && let Some(git) = self
                .workspace
                .read(cx)
                .remote_snapshot(&make_prefixed_id(&owner.id, project))
                .and_then(|p| p.git_status.as_ref())
        {
            row = row.child(format!(
                "{} · +{} −{} · ahead {} / behind {}{}",
                git.branch.as_deref().unwrap_or("Unknown branch"),
                git.lines_added,
                git.lines_removed,
                git.ahead
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into()),
                git.behind
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "?".into()),
                if owner.connected {
                    ""
                } else {
                    " · last known"
                }
            ));
        }
        if attached
            && owner.connected
            && let (Some(project), Some(terminal)) = (project, terminal)
        {
            let connection = owner.id.clone();
            let completion = overview
                .attention
                .iter()
                .find(|e| {
                    e.kind == AttentionKind::Completion
                        && !e.read
                        && e.available
                        && e.source.project_id == project
                        && e.source.terminal_id == terminal
                })
                .cloned();
            row = row.child(
                button(format!("member-open-{index}"), "Open terminal").on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.reveal_work(
                            connection.clone(),
                            project.clone(),
                            terminal.clone(),
                            completion.clone(),
                            cx,
                        )
                    },
                )),
            );
        }
        if owner.connected {
            let conflict = member_owner(overview, &member).filter(|m| m.id != mission.id);
            let label = if attached {
                "Detach".into()
            } else if let Some(conflict) = conflict {
                format!("Move from ‘{}’ to this mission", conflict.title)
            } else {
                "Attach".into()
            };
            let connection = owner.id.clone();
            let id = mission.id.clone();
            let moving = conflict.is_some();
            row = row.child(
                button(format!("member-{attached}-{index}"), label).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let command = if attached {
                            MissionCommand::Detach {
                                mission_id: id.clone(),
                                member: member.clone(),
                            }
                        } else if moving {
                            MissionCommand::Move {
                                mission_id: id.clone(),
                                member: member.clone(),
                            }
                        } else {
                            MissionCommand::Attach {
                                mission_id: id.clone(),
                                member: member.clone(),
                            }
                        };
                        this.mission_command(&connection, command, cx);
                    },
                )),
            );
        }
        row.into_any_element()
    }
}

fn mission_members(mission: &Mission, overview: &WorkOverview) -> Vec<MissionMember> {
    mission
        .repository_ids
        .iter()
        .map(|id| MissionMember::Repository {
            project_id: id.clone(),
        })
        .chain(
            mission
                .worktree_ids
                .iter()
                .map(|id| MissionMember::Worktree {
                    project_id: id.clone(),
                }),
        )
        .chain(
            mission
                .conversations
                .iter()
                .map(|c| MissionMember::Conversation {
                    conversation: c.clone(),
                }),
        )
        .chain(
            overview
                .terminal_bindings
                .iter()
                .filter(|b| b.mission_id == mission.id)
                .map(|b| MissionMember::Terminal {
                    project_id: b.project_id.clone(),
                    terminal_id: b.terminal_id.clone(),
                }),
        )
        .collect()
}

fn member_owner<'a>(overview: &'a WorkOverview, member: &MissionMember) -> Option<&'a Mission> {
    match member {
        MissionMember::Repository { .. } => None,
        MissionMember::Worktree { project_id } => overview
            .missions
            .iter()
            .find(|m| m.worktree_ids.contains(project_id)),
        MissionMember::Conversation { conversation } => overview
            .missions
            .iter()
            .find(|m| m.conversations.contains(conversation)),
        MissionMember::Terminal {
            project_id,
            terminal_id,
        } => {
            if let Some(binding) = overview
                .terminal_bindings
                .iter()
                .find(|b| b.project_id == *project_id && b.terminal_id == *terminal_id)
            {
                return overview
                    .missions
                    .iter()
                    .find(|m| m.id == binding.mission_id);
            }
            None
        }
    }
}

fn mission_has_source(
    mission: &Mission,
    overview: &WorkOverview,
    episode: &AttentionEpisode,
) -> bool {
    if let Some(conversation) = &episode.source.conversation
        && let Some(owner) = member_owner(
            overview,
            &MissionMember::Conversation {
                conversation: conversation.clone(),
            },
        )
    {
        return owner.id == mission.id;
    }
    member_owner(
        overview,
        &MissionMember::Terminal {
            project_id: episode.source.project_id.clone(),
            terminal_id: episode.source.terminal_id.clone(),
        },
    )
    .is_some_and(|owner| owner.id == mission.id)
}

#[cfg(test)]
mod tests {
    use super::{Owner, member_owner, route_work_action, sorted_attention};
    use okena_core::{
        attention::{AttentionEpisode, AttentionKind, AttentionSource, ConversationId},
        mission::{
            ConversationAttachment, Mission, MissionLifecycle, MissionMember,
            TerminalMissionBinding, WorkOverview,
        },
    };

    #[gpui::test]
    fn mission_submission_retains_failed_draft_and_ignores_late_replies(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::{MissionDraft, MissionSaved};
        use gpui::AppContext;
        use okena_ui::simple_input::SimpleInputState;

        let title = cx.new(|cx| SimpleInputState::new(cx).default_value("My title"));
        let goal = cx.new(|cx| SimpleInputState::new(cx).default_value("My goal"));
        let submission = uuid::Uuid::new_v4();
        let mut draft = MissionDraft {
            connection_id: "b".into(),
            mission_id: None,
            title,
            goal,
            home: Some("repository".into()),
            member: None,
            error: None,
            submission: Some(submission),
        };
        for error in ["Already belongs to another mission", "HTTP request failed"] {
            draft.submission = Some(submission);
            assert!(
                draft
                    .finish_submission(submission, Err(error.into()))
                    .is_none()
            );
            assert_eq!(draft.error.as_deref(), Some(error));
            assert!(draft.submission.is_none());
            cx.update(|cx| {
                assert_eq!(draft.title.read(cx).value(), "My title");
                assert_eq!(draft.goal.read(cx).value(), "My goal");
            });
            assert_eq!(draft.home.as_deref(), Some("repository"));
        }
        let next = uuid::Uuid::new_v4();
        draft.submission = Some(next);
        assert!(
            draft
                .finish_submission(
                    submission,
                    Ok(MissionSaved {
                        mission_id: "old".into()
                    })
                )
                .is_none()
        );
        assert_eq!(draft.submission, Some(next));
        let selected = draft
            .finish_submission(
                next,
                Ok(MissionSaved {
                    mission_id: "created".into(),
                }),
            )
            .unwrap();
        assert_eq!(selected.connection_id, "b");
        assert_eq!(selected.mission_id, "created");
        assert!(draft.submission.is_none());
    }

    #[test]
    fn mission_response_requires_a_nonempty_mission_id() {
        use super::MissionSaved;
        for invalid in [
            serde_json::json!({"ok": true}),
            serde_json::json!({"mission_id": ""}),
            serde_json::json!({"mission_id": 1}),
        ] {
            assert!(MissionSaved::parse(invalid).is_err());
        }
        assert_eq!(
            MissionSaved::parse(serde_json::json!({"mission_id": "created"}))
                .unwrap()
                .mission_id,
            "created"
        );
    }

    fn episode(kind: AttentionKind, id: &str, created_at: u64) -> AttentionEpisode {
        AttentionEpisode {
            id: id.into(),
            revision: 1,
            source: AttentionSource {
                project_id: "p".into(),
                terminal_id: "t".into(),
                attachment_id: "attachment".into(),
                generation: 1,
                conversation: None,
            },
            kind,
            summary: "Result".into(),
            created_at,
            updated_at: created_at,
            available: true,
            read: false,
        }
    }

    #[test]
    fn worktree_submit_retains_explicit_mission_and_revalidates_owner() {
        use super::worktree_create_request;
        use okena_core::api::ActionRequest;
        use okena_workspace::state::{MissionSelection, WorkspaceData};

        let mut data = WorkspaceData::empty();
        for owner in ["a", "b"] {
            data.projects.push(
                serde_json::from_value(serde_json::json!({
                    "id": format!("remote:{owner}:same-project"), "name": "Repository",
                    "path": "/repository", "layout": null, "connection_id": owner
                }))
                .unwrap(),
            );
        }
        let mission = Mission {
            id: "same-mission".into(),
            title: "Mission".into(),
            goal: None,
            home_project_id: None,
            created_at: 1,
            lifecycle: MissionLifecycle::Active,
            repository_ids: vec![],
            worktree_ids: vec![],
            conversations: vec![],
            pull_requests: vec![],
        };
        let mut owners: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|id| Owner {
                id: id.into(),
                name: id.into(),
                connected: true,
                overview: Some(WorkOverview {
                    missions: vec![mission.clone()],
                    ..Default::default()
                }),
            })
            .collect();
        let captured = MissionSelection {
            connection_id: "b".into(),
            mission_id: "same-mission".into(),
        };
        let action = worktree_create_request(
            &data,
            &owners,
            Some(&captured),
            "remote:b:same-project",
            "feature",
            true,
        )
        .unwrap();
        assert!(
            matches!(action, ActionRequest::CreateWorktree { project_id, branch, create_branch: true, mission_id: Some(id) }
            if project_id == "remote:b:same-project" && branch == "feature" && id == "same-mission")
        );
        assert!(
            worktree_create_request(
                &data,
                &owners,
                Some(&captured),
                "remote:a:same-project",
                "feature",
                true
            )
            .is_none()
        );
        assert!(matches!(
            worktree_create_request(
                &data,
                &owners,
                None,
                "remote:b:same-project",
                "ordinary",
                false
            ),
            Some(ActionRequest::CreateWorktree {
                mission_id: None,
                ..
            })
        ));
        owners[1].connected = false;
        assert!(
            worktree_create_request(
                &data,
                &owners,
                Some(&captured),
                "remote:b:same-project",
                "feature",
                true
            )
            .is_none()
        );
        owners[1].connected = true;
        owners[1].overview.as_mut().unwrap().missions.clear();
        assert!(
            worktree_create_request(
                &data,
                &owners,
                Some(&captured),
                "remote:b:same-project",
                "feature",
                true
            )
            .is_none()
        );
        owners[1].overview.as_mut().unwrap().missions.push(mission);
        data.projects
            .retain(|project| project.connection_id.as_deref() != Some("b"));
        assert!(
            worktree_create_request(
                &data,
                &owners,
                Some(&captured),
                "remote:b:same-project",
                "feature",
                true
            )
            .is_none()
        );
    }

    #[test]
    fn work_actions_mutate_only_the_selected_owner_with_colliding_raw_ids() {
        use okena_core::{
            agent_status::{AgentLifecycle, AgentStatus},
            api::ActionRequest,
            mission::MissionCommand,
        };
        use okena_workspace::state::WorkspaceData;
        use std::collections::HashMap;

        let mut data = WorkspaceData::empty();
        data.projects.push(
            serde_json::from_value(serde_json::json!({
                "id": "same-project", "name": "Repository", "path": "/repository", "layout": null
            }))
            .unwrap(),
        );
        data.missions.push(Mission {
            id: "same-mission".into(),
            title: "Original".into(),
            goal: None,
            home_project_id: None,
            created_at: 1,
            lifecycle: MissionLifecycle::Active,
            repository_ids: vec![],
            worktree_ids: vec![],
            conversations: vec![],
            pull_requests: vec![],
        });
        let episode_id = "00000000-0000-0000-0000-000000000001";
        assert!(data.attention.record(
            AttentionSource {
                project_id: "same-project".into(),
                terminal_id: "same-terminal".into(),
                attachment_id: "same-attachment".into(),
                generation: 1,
                conversation: None
            },
            Some(&AgentStatus::new(AgentLifecycle::Done)),
            1,
            episode_id.into()
        ));
        let mut daemons = HashMap::from([("a".to_string(), data.clone()), ("b".to_string(), data)]);
        let mut owners: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|id| Owner {
                id: id.into(),
                name: id.into(),
                connected: true,
                overview: Some(okena_workspace::missions::work_overview(&daemons[id])),
            })
            .collect();

        let mut dispatch = |connection: &str, action| {
            let owner = daemons
                .get_mut(connection)
                .expect("dispatch must retain the selected connection");
            match action {
                ActionRequest::Mission { command } => {
                    okena_workspace::missions::apply_command(owner, command).unwrap();
                }
                ActionRequest::AcknowledgeAttention {
                    episode_id,
                    revision,
                    dismiss,
                } => {
                    assert!(owner.attention.acknowledge(&episode_id, revision, dismiss));
                }
                _ => panic!("unexpected work action"),
            }
        };
        assert!(route_work_action(
            &owners,
            "b",
            ActionRequest::Mission {
                command: MissionCommand::Edit {
                    mission_id: "same-mission".into(),
                    title: "Changed on B".into(),
                    goal: None,
                    home_project_id: Some("same-project".into())
                }
            },
            &mut dispatch
        ));
        assert!(route_work_action(
            &owners,
            "a",
            ActionRequest::Mission {
                command: MissionCommand::Attach {
                    mission_id: "same-mission".into(),
                    member: MissionMember::Repository {
                        project_id: "same-project".into()
                    }
                }
            },
            &mut dispatch
        ));
        assert!(route_work_action(
            &owners,
            "b",
            ActionRequest::AcknowledgeAttention {
                episode_id: episode_id.into(),
                revision: 1,
                dismiss: false
            },
            &mut dispatch
        ));

        assert_eq!(daemons["a"].missions[0].title, "Original");
        assert_eq!(daemons["b"].missions[0].title, "Changed on B");
        assert_eq!(daemons["a"].missions[0].repository_ids, ["same-project"]);
        assert!(daemons["b"].missions[0].repository_ids.is_empty());
        assert_eq!(daemons["a"].attention.episodes().len(), 1);
        assert!(daemons["b"].attention.episodes().is_empty());

        owners[1].connected = false;
        assert!(!route_work_action(
            &owners,
            "b",
            ActionRequest::Mission {
                command: MissionCommand::SetLifecycle {
                    mission_id: "same-mission".into(),
                    lifecycle: MissionLifecycle::Done
                }
            },
            |_, _| panic!("disconnected mutation must not dispatch")
        ));
        owners[0].overview = None;
        assert!(!route_work_action(
            &owners,
            "a",
            ActionRequest::AcknowledgeAttention {
                episode_id: episode_id.into(),
                revision: 1,
                dismiss: false
            },
            |_, _| panic!("old daemon must not receive a fake acknowledgment")
        ));
        assert_eq!(daemons["b"].missions[0].lifecycle, MissionLifecycle::Active);
        assert_eq!(daemons["a"].attention.episodes().len(), 1);
    }

    #[test]
    fn inbox_orders_kinds_age_and_colliding_connections_stably() {
        let mut revised = episode(AttentionKind::InputNeeded, "same", 3);
        revised.revision = 4;
        revised.updated_at = 100;
        let ordered = sorted_attention([
            (
                "b".into(),
                vec![
                    episode(AttentionKind::Completion, "early", 1),
                    revised.clone(),
                ],
            ),
            (
                "a".into(),
                vec![revised, episode(AttentionKind::InputNeeded, "old", 2)],
            ),
        ]);
        let ids: Vec<_> = ordered
            .iter()
            .map(|(owner, e)| (owner.as_str(), e.id.as_str()))
            .collect();
        assert_eq!(
            ids,
            [("a", "old"), ("a", "same"), ("b", "same"), ("b", "early")]
        );
    }

    #[test]
    fn conversation_owner_overrides_worktree_and_survives_member_disappearance() {
        let conversation = ConversationId {
            agent: "claude-code".into(),
            session_id: "session".into(),
        };
        let mission = |id: &str| Mission {
            id: id.into(),
            title: id.into(),
            goal: None,
            home_project_id: None,
            created_at: 1,
            lifecycle: MissionLifecycle::Active,
            repository_ids: vec![],
            worktree_ids: vec![],
            conversations: vec![],
            pull_requests: vec![],
        };
        let mut worktree = mission("default");
        worktree.worktree_ids.push("p".into());
        let mut explicit = mission("explicit");
        explicit.conversations.push(conversation.clone());
        let mut overview = WorkOverview {
            missions: vec![worktree, explicit],
            conversations: vec![ConversationAttachment {
                project_id: "p".into(),
                terminal_id: "t".into(),
                conversation: conversation.clone(),
            }],
            terminal_bindings: vec![TerminalMissionBinding {
                project_id: "p".into(),
                terminal_id: "t".into(),
                mission_id: "explicit".into(),
            }],
            ..Default::default()
        };
        let terminal = MissionMember::Terminal {
            project_id: "p".into(),
            terminal_id: "t".into(),
        };
        assert_eq!(member_owner(&overview, &terminal).unwrap().id, "explicit");
        overview.conversations.clear();
        overview.terminal_bindings.clear();
        assert_eq!(
            member_owner(&overview, &MissionMember::Conversation { conversation })
                .unwrap()
                .id,
            "explicit"
        );
        assert!(
            member_owner(&overview, &terminal).is_none(),
            "effective binding must come from the owner, including exclusions"
        );
    }
}
