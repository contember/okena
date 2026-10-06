use crate::commands::fetch_state;
use crate::parser::{MissionCmd, MissionMemberArgs};
use crate::{api_action, ensure_token, mission_context, resolve};
use okena_core::api::{ActionRequest, ApiProject, StateResponse};
use okena_core::attention::ConversationId;
use okena_core::mission::{
    ConversationAttachment, Mission, MissionCommand, MissionContextProject, MissionMember,
    TerminalMissionBinding, WorkOverview, validate_text,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub fn run(command: MissionCmd) -> i32 {
    if let MissionCmd::Context(args) = command {
        return mission_context::run(args);
    }
    match execute(command) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Mission: {error}");
            1
        }
    }
}

fn execute(command: MissionCmd) -> Result<(), String> {
    let token = ensure_token()?;
    let state = fetch_state(&token)?;
    let overview = work_overview(&state)?;
    match &command {
        MissionCmd::List { json, quiet } => {
            if *json {
                print_json(&overview.missions)?;
            } else {
                for mission in &overview.missions {
                    if *quiet {
                        println!("{}", mission.id);
                    } else {
                        println!("{}\t{}\t{}", mission.id, lifecycle(mission), mission.title);
                    }
                }
            }
        }
        MissionCmd::Show { mission, json } => {
            let details = mission_details(&state, resolve_mission(overview, mission)?);
            if *json {
                print_json(&details)?;
            } else {
                print_details(&details);
            }
        }
        _ => {
            let current_terminal = std::env::var("OKENA_TERMINAL_ID").ok();
            let (mutation, json) = build_mutation(&state, &command, current_terminal.as_deref())?;
            let body = serde_json::to_string(&ActionRequest::Mission { command: mutation })
                .map_err(|e| e.to_string())?;
            let response = api_action(&token, &body)?;
            let result: MissionResult = serde_json::from_str(&response)
                .map_err(|e| format!("Invalid mission response: {e}"))?;
            if json {
                print_json(&result)?;
            } else {
                println!("{}", result.mission_id);
            }
        }
    }
    Ok(())
}

fn work_overview(state: &StateResponse) -> Result<&WorkOverview, String> {
    state
        .work_overview
        .as_ref()
        .ok_or_else(|| "This daemon does not advertise mission support; upgrade the daemon".into())
}

fn resolve_mission<'a>(overview: &'a WorkOverview, filter: &str) -> Result<&'a Mission, String> {
    if let Some(mission) = overview.missions.iter().find(|m| m.id == filter) {
        return Ok(mission);
    }
    let mut matches = overview
        .missions
        .iter()
        .filter(|m| m.title.eq_ignore_ascii_case(filter));
    let mission = matches
        .next()
        .ok_or_else(|| format!("Mission not found: {filter}"))?;
    if matches.next().is_some() {
        return Err(format!(
            "Ambiguous mission title: {filter}; use an exact mission ID"
        ));
    }
    Ok(mission)
}

fn resolve_member(
    state: &StateResponse,
    args: &MissionMemberArgs,
    current_terminal: Option<&str>,
) -> Result<Option<MissionMember>, String> {
    let terminal = if args.current_terminal {
        Some(current_terminal.filter(|id| !id.is_empty()).ok_or(
            "Not in an Okena terminal; supply --terminal <ADDRESS> or an explicit conversation",
        )?)
    } else {
        args.terminal.as_deref()
    };
    if let Some(terminal) = terminal {
        let (project_id, terminal_id) = resolve::resolve_terminal(state, terminal)?;
        return Ok(Some(MissionMember::Terminal {
            project_id,
            terminal_id,
        }));
    }
    if let Some(repository) = &args.repository {
        let project = resolve::resolve_project(state, repository)?;
        if project.worktree_info.is_some() {
            return Err("Selected project is a worktree; use --worktree".into());
        }
        return Ok(Some(MissionMember::Repository {
            project_id: project.id.clone(),
        }));
    }
    if let Some(worktree) = &args.worktree {
        let project = resolve::resolve_project(state, worktree)?;
        if project.worktree_info.is_none() {
            return Err("Selected project is a repository; use --repository".into());
        }
        return Ok(Some(MissionMember::Worktree {
            project_id: project.id.clone(),
        }));
    }
    match (&args.agent, &args.session_id) {
        (Some(agent), Some(session_id)) => {
            let conversation = ConversationId {
                agent: agent.clone(),
                session_id: session_id.clone(),
            };
            if !conversation.is_valid() {
                return Err("Invalid agent conversation identity".into());
            }
            Ok(Some(MissionMember::Conversation { conversation }))
        }
        (None, None) => Ok(None),
        _ => Err("--agent and --session-id must be supplied together".into()),
    }
}

fn build_mutation(
    state: &StateResponse,
    command: &MissionCmd,
    current_terminal: Option<&str>,
) -> Result<(MissionCommand, bool), String> {
    let overview = work_overview(state)?;
    match command {
        MissionCmd::Create {
            title,
            goal,
            home_project,
            member,
            json,
        } => {
            validate_text(title, goal.as_deref())?;
            let home_project_id = home_project
                .as_ref()
                .map(|filter| resolve::resolve_project(state, filter).map(|p| p.id.clone()))
                .transpose()?;
            Ok((
                MissionCommand::Create {
                    title: title.clone(),
                    goal: goal.clone(),
                    home_project_id,
                    member: resolve_member(state, member, current_terminal)?,
                },
                *json,
            ))
        }
        MissionCmd::Attach(args) | MissionCmd::Detach(args) | MissionCmd::Move(args) => {
            let mission_id = resolve_mission(overview, &args.mission)?.id.clone();
            let member = resolve_member(state, &args.member, current_terminal)?
                .ok_or("Select --current-terminal, --terminal, --repository, --worktree or --agent with --session-id")?;
            let mutation = match command {
                MissionCmd::Attach(_) => MissionCommand::Attach { mission_id, member },
                MissionCmd::Detach(_) => MissionCommand::Detach { mission_id, member },
                _ => MissionCommand::Move { mission_id, member },
            };
            Ok((mutation, args.json))
        }
        _ => Err("Expected a mission mutation".into()),
    }
}

#[derive(Deserialize, Serialize)]
struct MissionResult {
    mission_id: String,
}

#[derive(Serialize)]
struct MissionDetails<'a> {
    mission: &'a Mission,
    home_project: Option<MissionContextProject>,
    projects: Vec<MissionContextProject>,
    terminal_bindings: Vec<&'a TerminalMissionBinding>,
    conversation_attachments: Vec<&'a ConversationAttachment>,
}

fn mission_details<'a>(state: &'a StateResponse, mission: &'a Mission) -> MissionDetails<'a> {
    let overview = state.work_overview.as_ref();
    let terminal_bindings: Vec<_> = overview
        .into_iter()
        .flat_map(|o| &o.terminal_bindings)
        .filter(|b| b.mission_id == mission.id)
        .collect();
    let conversation_attachments: Vec<_> = overview
        .into_iter()
        .flat_map(|o| &o.conversations)
        .filter(|a| mission.conversations.contains(&a.conversation))
        .collect();
    let project_ids: HashSet<_> = mission
        .repository_ids
        .iter()
        .chain(&mission.worktree_ids)
        .chain(terminal_bindings.iter().map(|b| &b.project_id))
        .chain(conversation_attachments.iter().map(|a| &a.project_id))
        .collect();
    let projects = state
        .projects
        .iter()
        .filter(|p| project_ids.contains(&p.id))
        .map(project_details)
        .collect();
    let home_project = mission.home_project_id.as_ref().and_then(|id| {
        state
            .projects
            .iter()
            .find(|p| &p.id == id)
            .map(project_details)
    });
    MissionDetails {
        mission,
        home_project,
        projects,
        terminal_bindings,
        conversation_attachments,
    }
}

fn project_details(project: &ApiProject) -> MissionContextProject {
    MissionContextProject {
        id: project.id.clone(),
        name: project.name.clone(),
        path: project.path.clone(),
        branch: project.git_status.as_ref().and_then(|s| s.branch.clone()),
        is_worktree: project.worktree_info.is_some(),
    }
}

fn lifecycle(mission: &Mission) -> &'static str {
    match mission.lifecycle {
        okena_core::mission::MissionLifecycle::Active => "active",
        okena_core::mission::MissionLifecycle::Done => "done",
        okena_core::mission::MissionLifecycle::Archived => "archived",
    }
}

fn print_json(value: &impl Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn quoted(value: &str) -> String {
    serde_json::Value::String(value.into()).to_string()
}

fn print_details(details: &MissionDetails<'_>) {
    let mission = details.mission;
    println!(
        "mission\t{}\t{}\t{}",
        mission.id,
        lifecycle(mission),
        quoted(&mission.title)
    );
    if let Some(goal) = &mission.goal {
        println!("goal\t{}", quoted(goal));
    }
    if let Some(home) = &mission.home_project_id {
        println!("home_project\t{home}");
    }
    for id in &mission.repository_ids {
        println!("repository\t{id}");
    }
    for id in &mission.worktree_ids {
        println!("worktree\t{id}");
    }
    for project in &details.projects {
        println!(
            "project\t{}\t{}\t{}\t{}",
            project.id,
            quoted(&project.name),
            quoted(&project.path),
            quoted(project.branch.as_deref().unwrap_or(""))
        );
    }
    for binding in &details.terminal_bindings {
        println!("terminal\t{}\t{}", binding.project_id, binding.terminal_id);
    }
    for conversation in &mission.conversations {
        println!(
            "conversation\t{}\t{}",
            conversation.agent, conversation.session_id
        );
    }
    for attachment in &details.conversation_attachments {
        println!(
            "attachment\t{}\t{}\t{}\t{}",
            attachment.conversation.agent,
            attachment.conversation.session_id,
            attachment.project_id,
            attachment.terminal_id
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Cli, Command};
    use clap::Parser;

    fn fixture() -> StateResponse {
        serde_json::from_value(serde_json::json!({
            "state_version":1, "focused_project_id":"p", "fullscreen_terminal":null,
            "projects":[
                {"id":"p", "name":"Repo", "path":"/repo", "show_in_overview":true,
                 "terminal_names":{"t":"shell"},
                 "layout":{"type":"terminal", "terminal_id":"t", "minimized":false, "detached":false}},
                {"id":"w", "name":"Checkout", "path":"/checkout", "show_in_overview":true,
                 "terminal_names":{}, "worktree_info":{"parent_project_id":"p"},
                 "layout":{"type":"terminal", "terminal_id":"tw", "minimized":false, "detached":false}}
            ],
            "work_overview":{"missions":[{
                "id":"m", "title":"Export", "goal":null, "home_project_id":"p",
                "created_at":1, "lifecycle":"active", "worktree_ids":["w"]
            }], "attention":[], "lost_transitions":0, "conversations":[], "terminal_bindings":[]}
        })).unwrap()
    }

    fn command(args: &[&str]) -> MissionCmd {
        let cli = Cli::try_parse_from(["okena", "mission"].into_iter().chain(args.iter().copied()))
            .unwrap();
        let Command::Mission { cmd } = cli.command else {
            panic!("expected mission command");
        };
        cmd
    }

    fn mutation(state: &StateResponse, args: &[&str], terminal: Option<&str>) -> serde_json::Value {
        let (command, _) = build_mutation(state, &command(args), terminal).unwrap();
        serde_json::to_value(command).unwrap()
    }

    #[test]
    fn mission_resolution_rejects_ambiguous_titles_but_prefers_exact_ids() {
        let mut state = fixture();
        let overview = state.work_overview.as_mut().unwrap();
        assert_eq!(resolve_mission(overview, "export").unwrap().id, "m");
        let mut duplicate = overview.missions[0].clone();
        duplicate.id = "other".into();
        overview.missions.push(duplicate);
        assert!(
            resolve_mission(overview, "Export")
                .unwrap_err()
                .contains("Ambiguous")
        );
        assert_eq!(resolve_mission(overview, "m").unwrap().id, "m");
        assert!(resolve_mission(overview, "missing").is_err());
        overview.missions[1].title = "m".into();
        assert_eq!(resolve_mission(overview, "m").unwrap().id, "m");
    }

    #[test]
    fn create_keeps_home_context_separate_from_optional_membership() {
        let state = fixture();
        let request = mutation(
            &state,
            &[
                "create",
                "Review",
                "--goal",
                "Review changes",
                "--home-project",
                "repo",
            ],
            Some("t"),
        );
        assert_eq!(
            request,
            serde_json::json!({
                "operation":"create", "title":"Review", "goal":"Review changes",
                "home_project_id":"p", "member":null
            })
        );
        let request = mutation(
            &state,
            &["create", "Review", "--current-terminal"],
            Some("tw"),
        );
        assert_eq!(
            request["member"],
            serde_json::json!({"kind":"terminal", "project_id":"w", "terminal_id":"tw"})
        );
        assert!(build_mutation(&state, &command(&["create", " "]), None).is_err());
        assert!(
            build_mutation(
                &state,
                &command(&["create", "Review", "--goal", &"x".repeat(4097)]),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn current_terminal_uses_the_callers_pane_never_the_focused_project() {
        let state = fixture();
        let args = ["attach", "Export", "--current-terminal"];
        let request = mutation(&state, &args, Some("tw"));
        assert_eq!(
            request["member"],
            serde_json::json!({"kind":"terminal", "project_id":"w", "terminal_id":"tw"})
        );
        for terminal in [None, Some(""), Some("missing")] {
            assert!(build_mutation(&state, &command(&args), terminal).is_err());
        }
    }

    #[test]
    fn membership_operations_use_existing_terminal_addressing() {
        let state = fixture();
        for verb in ["attach", "detach", "move"] {
            for address in ["t", "Repo/shell", "Repo:0"] {
                let request = mutation(&state, &[verb, "export", "--terminal", address], None);
                assert_eq!(request["operation"], verb);
                assert_eq!(request["mission_id"], "m");
                assert_eq!(
                    request["member"],
                    serde_json::json!({"kind":"terminal", "project_id":"p", "terminal_id":"t"})
                );
            }
        }
    }

    #[test]
    fn repository_and_worktree_selectors_preserve_their_different_contracts() {
        let state = fixture();
        assert_eq!(
            mutation(&state, &["attach", "m", "--repository", "/repo"], None)["member"],
            serde_json::json!({"kind":"repository", "project_id":"p"})
        );
        assert_eq!(
            mutation(&state, &["attach", "m", "--worktree", "checkout"], None)["member"],
            serde_json::json!({"kind":"worktree", "project_id":"w"})
        );
        for args in [
            ["attach", "m", "--repository", "Checkout"],
            ["attach", "m", "--worktree", "Repo"],
        ] {
            assert!(build_mutation(&state, &command(&args), None).is_err());
        }
    }

    #[test]
    fn conversation_selector_uses_explicit_identity_without_a_terminal() {
        let state = fixture();
        let session = "00000000-0000-0000-0000-000000000001";
        let request = mutation(
            &state,
            &[
                "attach",
                "m",
                "--agent",
                "claude-code",
                "--session-id",
                session,
            ],
            None,
        );
        assert_eq!(
            request["member"],
            serde_json::json!({"kind":"conversation", "conversation":{"agent":"claude-code", "session_id":session}})
        );
        assert!(
            build_mutation(
                &state,
                &command(&[
                    "attach",
                    "m",
                    "--agent",
                    "claude-code",
                    "--session-id",
                    "../invalid"
                ]),
                Some("t")
            )
            .is_err()
        );
    }

    #[test]
    fn older_daemons_are_not_treated_as_an_empty_mission_list() {
        let mut state = fixture();
        state.work_overview = None;
        assert!(work_overview(&state).is_err());
        assert!(build_mutation(&state, &command(&["create", "Review"]), None).is_err());
    }

    #[test]
    fn show_is_unbounded_and_includes_resolved_panes_and_retained_conversations() {
        let mut state = fixture();
        let overview = state.work_overview.as_mut().unwrap();
        overview.terminal_bindings.push(TerminalMissionBinding {
            mission_id: "m".into(),
            project_id: "p".into(),
            terminal_id: "t".into(),
        });
        for n in 0..20 {
            overview.missions[0].conversations.push(ConversationId {
                agent: "claude-code".into(),
                session_id: format!("00000000-0000-0000-0000-{n:012x}"),
            });
        }
        overview.conversations.push(ConversationAttachment {
            project_id: "w".into(),
            terminal_id: "tw".into(),
            conversation: overview.missions[0].conversations[0].clone(),
        });
        let details = mission_details(&state, &work_overview(&state).unwrap().missions[0]);
        assert_eq!(details.projects.len(), 2);
        assert_eq!(details.home_project.as_ref().unwrap().path, "/repo");
        assert_eq!(details.terminal_bindings.len(), 1);
        assert_eq!(details.conversation_attachments.len(), 1);
        let json = serde_json::to_value(details).unwrap();
        assert_eq!(
            json["mission"]["conversations"].as_array().unwrap().len(),
            20
        );
        assert_eq!(json["mission"]["worktree_ids"], serde_json::json!(["w"]));
    }
}
