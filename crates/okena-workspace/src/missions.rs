use crate::context::WorkspaceCx;
use crate::state::{LayoutNode, Workspace, WorkspaceData};
use okena_core::attention::ConversationId;
use okena_core::mission::{
    ConversationAttachment, Mission, MissionCommand, MissionLifecycle, MissionMember,
    MissionPullRequest, TerminalMissionBinding, WorkOverview,
};

#[derive(Clone, Debug)]
pub struct MissionPrObservation {
    pub snapshot: MissionPullRequest,
    pub pr_fetched: bool,
    pub ci_fetched: bool,
}

impl Workspace {
    pub fn validate_worktree_mission(
        &self,
        parent_project_id: &str,
        mission_id: Option<&str>,
    ) -> Result<(), String> {
        local_project(&self.data, parent_project_id)?;
        if let Some(id) = mission_id
            && !self.data.missions.iter().any(|m| m.id == id)
        {
            return Err("Mission not found on this daemon".into());
        }
        Ok(())
    }
    pub fn execute_mission(
        &mut self,
        command: MissionCommand,
        cx: &mut impl WorkspaceCx,
    ) -> Result<String, String> {
        // Validation and conflict checks are atomic, including Create with an initial member.
        let mut data = self.data.clone();
        let id = apply_command(&mut data, command)?;
        self.data = data;
        self.notify_data(cx);
        Ok(id)
    }

    /// The caller supplies canonical host/repository identity from git.
    /// `None` invalidates this checkout's observation, retaining last-known PR data.
    pub fn record_mission_pr(
        &mut self,
        project_id: &str,
        observation: Option<MissionPrObservation>,
        cx: &mut impl WorkspaceCx,
    ) -> Result<(), String> {
        local_project(&self.data, project_id)?;
        if let Some(ref pr) = observation
            && !valid_pr(&pr.snapshot)
        {
            return Err("Invalid pull request identity".into());
        }
        for mission in &mut self.data.missions {
            for pr in &mut mission.pull_requests {
                pr.source_project_ids.retain(|id| id != project_id);
                pr.available = !pr.source_project_ids.is_empty();
            }
            if mission
                .worktree_ids
                .iter()
                .chain(&mission.repository_ids)
                .any(|id| id == project_id)
                && let Some(observation) = &observation
            {
                let pr = &observation.snapshot;
                if let Some(existing) = mission
                    .pull_requests
                    .iter_mut()
                    .find(|p| p.identity == pr.identity)
                {
                    if observation.pr_fetched {
                        existing.info = pr.info.clone();
                    }
                    if observation.ci_fetched {
                        existing.ci = pr.ci.clone();
                    }
                    if observation.pr_fetched || observation.ci_fetched {
                        existing.observed_at = existing.observed_at.max(pr.observed_at);
                    }
                    if pr.available {
                        existing.source_project_ids.push(project_id.into());
                    }
                    existing.available = !existing.source_project_ids.is_empty();
                } else {
                    let mut pr = pr.clone();
                    pr.source_project_ids = if pr.available {
                        vec![project_id.into()]
                    } else {
                        Vec::new()
                    };
                    mission.pull_requests.push(pr);
                }
            }
        }
        self.notify_data(cx);
        Ok(())
    }
}

fn valid_pr(pr: &MissionPullRequest) -> bool {
    !pr.identity.host.is_empty()
        && pr.identity.host.len() <= 255
        && !pr.identity.repository.is_empty()
        && pr.identity.repository.len() <= 512
        && pr.identity.number == pr.info.number
        && pr.identity.number > 0
        && !pr.info.url.is_empty()
        && pr.info.url.len() <= 4096
}

fn local_project<'a>(
    data: &'a WorkspaceData,
    id: &str,
) -> Result<&'a crate::state::ProjectData, String> {
    data.projects
        .iter()
        .find(|p| p.id == id && p.connection_id.is_none())
        .ok_or_else(|| "Project is missing or belongs to another daemon".into())
}

pub fn conversation_mission<'a>(
    data: &'a WorkspaceData,
    conversation: &ConversationId,
) -> Option<&'a str> {
    data.missions
        .iter()
        .find(|m| m.conversations.contains(conversation))
        .map(|m| m.id.as_str())
}

pub fn terminal_mission<'a>(
    data: &'a WorkspaceData,
    project_id: &str,
    terminal_id: &str,
) -> Option<&'a str> {
    let project = data.projects.iter().find(|p| p.id == project_id)?;
    let layout = project.layout.as_ref()?;
    let leaf = layout.get_at_path(&layout.find_terminal_path(terminal_id)?)?;
    if matches!(
        leaf,
        LayoutNode::Terminal {
            mission_excluded: true,
            ..
        }
    ) {
        return None;
    }
    if let Some(session) = project.agent_sessions.get(terminal_id) {
        let conversation = ConversationId::from(session);
        if data.mission_excluded_conversations.contains(&conversation) {
            return None;
        }
        if let Some(id) = conversation_mission(data, &conversation) {
            return Some(id);
        }
    }
    terminal_default_mission(data, project_id, terminal_id)
}

fn terminal_default_mission<'a>(
    data: &'a WorkspaceData,
    project_id: &str,
    terminal_id: &str,
) -> Option<&'a str> {
    let project = data.projects.iter().find(|p| p.id == project_id)?;
    let layout = project.layout.as_ref()?;
    let leaf = layout.get_at_path(&layout.find_terminal_path(terminal_id)?)?;
    if matches!(
        leaf,
        LayoutNode::Terminal {
            mission_excluded: true,
            ..
        }
    ) {
        return None;
    }
    if let LayoutNode::Terminal {
        mission_id: Some(id),
        ..
    } = leaf
    {
        return Some(id);
    }
    data.missions
        .iter()
        .find(|m| m.worktree_ids.iter().any(|id| id == project_id))
        .map(|m| m.id.as_str())
}

pub fn promote_conversation(
    data: &mut WorkspaceData,
    project_id: &str,
    terminal_id: &str,
    conversation: ConversationId,
) -> bool {
    if !conversation.is_valid()
        || data.mission_excluded_conversations.contains(&conversation)
        || conversation_mission(data, &conversation).is_some()
    {
        return false;
    }
    let excluded = data
        .projects
        .iter()
        .find(|p| p.id == project_id)
        .and_then(|p| p.layout.as_ref())
        .and_then(|l| {
            l.find_terminal_path(terminal_id)
                .and_then(|path| l.get_at_path(&path))
        })
        .is_some_and(|leaf| {
            matches!(
                leaf,
                LayoutNode::Terminal {
                    mission_excluded: true,
                    ..
                }
            )
        });
    if excluded {
        data.mission_excluded_conversations.push(conversation);
        return true;
    }
    let Some(id) = terminal_default_mission(data, project_id, terminal_id).map(str::to_owned)
    else {
        return false;
    };
    if let Some(mission) = data.missions.iter_mut().find(|m| m.id == id) {
        mission.conversations.push(conversation);
        return true;
    }
    false
}

pub fn apply_command(data: &mut WorkspaceData, command: MissionCommand) -> Result<String, String> {
    match command {
        MissionCommand::Create {
            title,
            goal,
            home_project_id,
            member,
        } => {
            validate_details(data, &title, goal.as_deref(), home_project_id.as_deref())?;
            if let Some(member) = &member {
                validate_member(data, member)?;
            }
            let id = uuid::Uuid::new_v4().to_string();
            data.missions.push(Mission {
                id: id.clone(),
                title,
                goal,
                home_project_id,
                created_at: okena_state::now_unix_seconds(),
                lifecycle: MissionLifecycle::Active,
                repository_ids: Vec::new(),
                worktree_ids: Vec::new(),
                conversations: Vec::new(),
                pull_requests: Vec::new(),
            });
            if let Some(member) = member
                && let Err(error) = attach(data, &id, &member, false)
            {
                data.missions.retain(|m| m.id != id);
                return Err(error);
            }
            Ok(id)
        }
        MissionCommand::Edit {
            mission_id,
            title,
            goal,
            home_project_id,
        } => {
            validate_details(data, &title, goal.as_deref(), home_project_id.as_deref())?;
            let mission = mission_mut(data, &mission_id)?;
            mission.title = title;
            mission.goal = goal;
            mission.home_project_id = home_project_id;
            Ok(mission_id)
        }
        MissionCommand::SetLifecycle {
            mission_id,
            lifecycle,
        } => {
            mission_mut(data, &mission_id)?.lifecycle = lifecycle;
            Ok(mission_id)
        }
        MissionCommand::Attach { mission_id, member } => {
            attach(data, &mission_id, &member, false)?;
            Ok(mission_id)
        }
        MissionCommand::Move { mission_id, member } => {
            attach(data, &mission_id, &member, true)?;
            Ok(mission_id)
        }
        MissionCommand::Detach { mission_id, member } => {
            mission_mut(data, &mission_id)?;
            validate_member(data, &member)?;
            if let MissionMember::Terminal {
                project_id,
                terminal_id,
            } = &member
                && local_project(data, project_id)?
                    .layout
                    .as_ref()
                    .and_then(|layout| {
                        layout
                            .find_terminal_path(terminal_id)
                            .and_then(|path| layout.get_at_path(&path))
                    })
                    .is_some_and(|leaf| {
                        matches!(
                            leaf,
                            LayoutNode::Terminal {
                                mission_excluded: true,
                                ..
                            }
                        )
                    })
            {
                return Ok(mission_id);
            }
            if let Some(current) = owner(data, &member)
                && current != mission_id
            {
                return Err("Member belongs to another mission".into());
            }
            detach(data, &mission_id, &member);
            Ok(mission_id)
        }
    }
}

fn mission_mut<'a>(data: &'a mut WorkspaceData, id: &str) -> Result<&'a mut Mission, String> {
    data.missions
        .iter_mut()
        .find(|m| m.id == id)
        .ok_or_else(|| "Mission not found on this daemon".into())
}

fn validate_details(
    data: &WorkspaceData,
    title: &str,
    goal: Option<&str>,
    home: Option<&str>,
) -> Result<(), String> {
    okena_core::mission::validate_text(title, goal)?;
    if let Some(id) = home {
        local_project(data, id)?;
    }
    Ok(())
}

fn validate_member(data: &WorkspaceData, member: &MissionMember) -> Result<(), String> {
    match member {
        MissionMember::Repository { project_id } => {
            if local_project(data, project_id)?.worktree_info.is_some() {
                return Err("Use worktree membership for a worktree".into());
            }
        }
        MissionMember::Worktree { project_id } => {
            if local_project(data, project_id)?.worktree_info.is_none() {
                return Err("Project is not a worktree".into());
            }
        }
        MissionMember::Terminal {
            project_id,
            terminal_id,
        } => {
            if local_project(data, project_id)?
                .layout
                .as_ref()
                .and_then(|l| l.find_terminal_path(terminal_id))
                .is_none()
            {
                return Err("Terminal not found in project".into());
            }
        }
        MissionMember::Conversation { conversation } => {
            if !conversation.is_valid()
                || !data
                    .agent_session_history
                    .sessions()
                    .iter()
                    .any(|s| ConversationId::from(s) == *conversation)
            {
                return Err("Unknown conversation on this daemon".into());
            }
        }
    }
    Ok(())
}

fn owner<'a>(data: &'a WorkspaceData, member: &MissionMember) -> Option<&'a str> {
    match member {
        MissionMember::Repository { .. } => None,
        MissionMember::Worktree { project_id } => data
            .missions
            .iter()
            .find(|m| m.worktree_ids.contains(project_id))
            .map(|m| m.id.as_str()),
        MissionMember::Conversation { conversation } => conversation_mission(data, conversation),
        MissionMember::Terminal {
            project_id,
            terminal_id,
        } => terminal_mission(data, project_id, terminal_id),
    }
}

fn attach(
    data: &mut WorkspaceData,
    id: &str,
    member: &MissionMember,
    moving: bool,
) -> Result<(), String> {
    mission_mut(data, id)?;
    validate_member(data, member)?;
    let assigned_conversation = match member {
        MissionMember::Terminal {
            project_id,
            terminal_id,
        } => local_project(data, project_id)?
            .agent_sessions
            .get(terminal_id)
            .and_then(|s| conversation_mission(data, &ConversationId::from(s))),
        _ => None,
    };
    if let Some(previous) = assigned_conversation
        .or_else(|| owner(data, member))
        .map(str::to_owned)
        && previous != id
    {
        if !moving {
            return Err("Member already belongs to another mission; use Move".into());
        }
        detach(data, &previous, member);
    }
    match member {
        MissionMember::Repository { project_id } => {
            let mission = mission_mut(data, id)?;
            if !mission.repository_ids.contains(project_id) {
                mission.repository_ids.push(project_id.clone());
            }
        }
        MissionMember::Worktree { project_id } => {
            let mission = mission_mut(data, id)?;
            if !mission.worktree_ids.contains(project_id) {
                mission.worktree_ids.push(project_id.clone());
            }
            let sessions: Vec<_> = local_project(data, project_id)?
                .agent_sessions
                .iter()
                .map(|(terminal, session)| (terminal.clone(), ConversationId::from(session)))
                .collect();
            for (terminal, conversation) in sessions {
                promote_conversation(data, project_id, &terminal, conversation);
            }
        }
        MissionMember::Conversation { conversation } => {
            clear_conversation_exclusion(data, conversation);
            let mission = mission_mut(data, id)?;
            if !mission.conversations.contains(conversation) {
                mission.conversations.push(conversation.clone());
            }
        }
        MissionMember::Terminal {
            project_id,
            terminal_id,
        } => {
            set_leaf_binding(data, project_id, terminal_id, Some(id.into()));
            let conversation = local_project(data, project_id)?
                .agent_sessions
                .get(terminal_id)
                .map(ConversationId::from);
            if let Some(conversation) = conversation {
                clear_conversation_exclusion(data, &conversation);
                promote_conversation(data, project_id, terminal_id, conversation);
            }
        }
    }
    Ok(())
}

fn set_leaf_binding(
    data: &mut WorkspaceData,
    project_id: &str,
    terminal_id: &str,
    binding: Option<String>,
) {
    if let Some(layout) = data
        .projects
        .iter_mut()
        .find(|p| p.id == project_id)
        .and_then(|p| p.layout.as_mut())
        && let Some(path) = layout.find_terminal_path(terminal_id)
        && let Some(LayoutNode::Terminal {
            mission_id,
            mission_excluded,
            ..
        }) = layout.get_at_path_mut(&path)
    {
        *mission_excluded = binding.is_none();
        *mission_id = binding;
    }
}

fn detach(data: &mut WorkspaceData, id: &str, member: &MissionMember) {
    let conversation = match member {
        MissionMember::Conversation { conversation } => Some(conversation.clone()),
        MissionMember::Terminal {
            project_id,
            terminal_id,
        } => data
            .projects
            .iter()
            .find(|p| p.id == *project_id)
            .and_then(|p| p.agent_sessions.get(terminal_id))
            .map(ConversationId::from),
        _ => None,
    };
    if let Some(mission) = data.missions.iter_mut().find(|m| m.id == id) {
        match member {
            MissionMember::Repository { project_id } => {
                mission.repository_ids.retain(|p| p != project_id)
            }
            MissionMember::Worktree { project_id } => {
                mission.worktree_ids.retain(|p| p != project_id)
            }
            _ => {}
        }
        if let Some(conversation) = &conversation {
            mission.conversations.retain(|c| c != conversation);
        }
    }
    if let MissionMember::Terminal {
        project_id,
        terminal_id,
    } = member
    {
        set_leaf_binding(data, project_id, terminal_id, None);
    }
    if let Some(conversation) = conversation {
        if !data.mission_excluded_conversations.contains(&conversation) {
            data.mission_excluded_conversations
                .push(conversation.clone());
        }
        for project in &mut data.projects {
            if let Some(layout) = &mut project.layout {
                clear_pending_binding(layout, id, &conversation);
                for (terminal_id, session) in &project.agent_sessions {
                    if ConversationId::from(session) == conversation
                        && let Some(path) = layout.find_terminal_path(terminal_id)
                        && let Some(LayoutNode::Terminal { mission_id, .. }) =
                            layout.get_at_path_mut(&path)
                        && mission_id.as_deref() == Some(id)
                    {
                        *mission_id = None;
                    }
                }
            }
        }
    }
}

fn clear_conversation_exclusion(data: &mut WorkspaceData, conversation: &ConversationId) {
    data.mission_excluded_conversations
        .retain(|c| c != conversation);
    for project in &mut data.projects {
        if let Some(layout) = &mut project.layout {
            clear_matching_leaf_exclusions(layout, &project.agent_sessions, conversation);
        }
    }
}

fn clear_matching_leaf_exclusions(
    node: &mut LayoutNode,
    sessions: &std::collections::HashMap<String, okena_core::agent_session::AgentSession>,
    conversation: &ConversationId,
) {
    match node {
        LayoutNode::Terminal {
            terminal_id,
            pending_agent_resume,
            mission_excluded,
            ..
        } => {
            let session = terminal_id
                .as_ref()
                .and_then(|id| sessions.get(id))
                .or(pending_agent_resume.as_ref());
            if session.is_some_and(|s| ConversationId::from(s) == *conversation) {
                *mission_excluded = false;
            }
        }
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for child in children {
                clear_matching_leaf_exclusions(child, sessions, conversation);
            }
        }
    }
}

fn clear_pending_binding(node: &mut LayoutNode, id: &str, conversation: &ConversationId) {
    match node {
        LayoutNode::Terminal {
            mission_id,
            pending_agent_resume,
            ..
        } => {
            if mission_id.as_deref() == Some(id)
                && pending_agent_resume
                    .as_ref()
                    .is_some_and(|s| ConversationId::from(s) == *conversation)
            {
                *mission_id = None;
            }
        }
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for child in children {
                clear_pending_binding(child, id, conversation);
            }
        }
    }
}

/// Removes live references only; conversation history and PR observations survive.
pub fn reconcile_membership(data: &mut WorkspaceData) {
    let local_ids: std::collections::HashSet<_> = data
        .projects
        .iter()
        .filter(|p| p.connection_id.is_none())
        .map(|p| p.id.clone())
        .collect();
    let removed_sources: Vec<_> = data
        .attention
        .observations
        .iter()
        .filter(|o| !local_ids.contains(&o.source.project_id))
        .map(|o| o.source.terminal_id.clone())
        .collect();
    for terminal in removed_sources {
        data.attention.invalidate_terminal(&terminal);
    }
    for mission in &mut data.missions {
        mission.repository_ids.retain(|id| local_ids.contains(id));
        mission.worktree_ids.retain(|id| local_ids.contains(id));
        for pr in &mut mission.pull_requests {
            pr.source_project_ids.retain(|id| {
                local_ids.contains(id)
                    && (mission.repository_ids.contains(id) || mission.worktree_ids.contains(id))
            });
            if pr.source_project_ids.is_empty() {
                pr.available = false;
            }
        }
    }
}

pub fn validate_persisted(data: &mut WorkspaceData) {
    let mut exclusions = std::collections::HashSet::new();
    data.mission_excluded_conversations
        .retain(|c| c.is_valid() && exclusions.insert(c.clone()));
    let mut ids = std::collections::HashSet::new();
    let mut conversations = std::collections::HashSet::new();
    let mut worktrees = std::collections::HashSet::new();
    data.missions.retain(|m| {
        okena_core::agent_session::is_uuid_like(&m.id)
            && okena_core::mission::validate_text(&m.title, m.goal.as_deref()).is_ok()
            && ids.insert(m.id.clone())
    });
    for mission in &mut data.missions {
        mission
            .conversations
            .retain(|c| c.is_valid() && !exclusions.contains(c) && conversations.insert(c.clone()));
        mission.worktree_ids.retain(|id| {
            data.projects
                .iter()
                .any(|p| p.id == *id && p.worktree_info.is_some())
                && worktrees.insert(id.clone())
        });
        mission.repository_ids.retain(|id| {
            data.projects
                .iter()
                .any(|p| p.id == *id && p.worktree_info.is_none())
        });
        let mut prs = std::collections::HashSet::new();
        mission.pull_requests.retain(|pr| {
            valid_pr(pr)
                && prs.insert((
                    pr.identity.host.clone(),
                    pr.identity.repository.clone(),
                    pr.identity.number,
                ))
        });
        for pr in &mut mission.pull_requests {
            pr.available = false;
            pr.source_project_ids.clear();
        }
    }
    for project in &mut data.projects {
        if let Some(layout) = &mut project.layout {
            visit_bindings(layout, &mut |binding| {
                if binding.as_ref().is_some_and(|id| !ids.contains(id)) {
                    *binding = None;
                }
            });
        }
    }
    reconcile_membership(data);
    let mut restored = Vec::new();
    let mut restored_exclusions = Vec::new();
    for project in data.projects.iter().filter(|p| p.connection_id.is_none()) {
        let default = data
            .missions
            .iter()
            .find(|m| m.worktree_ids.contains(&project.id))
            .map(|m| m.id.as_str());
        if let Some(layout) = &project.layout {
            collect_restored_members(
                layout,
                &project.agent_sessions,
                default,
                &mut restored,
                &mut restored_exclusions,
            );
        }
    }
    for conversation in restored_exclusions {
        if conversation_mission(data, &conversation).is_none()
            && !data.mission_excluded_conversations.contains(&conversation)
        {
            data.mission_excluded_conversations.push(conversation);
        }
    }
    for (mission_id, conversation) in restored {
        if !data.mission_excluded_conversations.contains(&conversation)
            && conversation_mission(data, &conversation).is_none()
            && let Some(mission) = data.missions.iter_mut().find(|m| m.id == mission_id)
        {
            mission.conversations.push(conversation);
        }
    }
}

fn collect_restored_members(
    node: &LayoutNode,
    sessions: &std::collections::HashMap<String, okena_core::agent_session::AgentSession>,
    default: Option<&str>,
    members: &mut Vec<(String, ConversationId)>,
    exclusions: &mut Vec<ConversationId>,
) {
    match node {
        LayoutNode::Terminal {
            terminal_id,
            mission_id,
            mission_excluded,
            pending_agent_resume,
            ..
        } => {
            let session = match terminal_id {
                Some(id) => sessions.get(id),
                None => pending_agent_resume.as_ref(),
            };
            if let Some(session) = session
                && session.is_valid()
            {
                if *mission_excluded {
                    exclusions.push(session.into());
                } else if let Some(mission_id) = mission_id.as_deref().or(default) {
                    members.push((mission_id.into(), session.into()));
                }
            }
        }
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for child in children {
                collect_restored_members(child, sessions, default, members, exclusions);
            }
        }
    }
}

fn visit_bindings(node: &mut LayoutNode, visit: &mut impl FnMut(&mut Option<String>)) {
    match node {
        LayoutNode::Terminal { mission_id, .. } => visit(mission_id),
        LayoutNode::Split { children, .. } | LayoutNode::Tabs { children, .. } => {
            for child in children {
                visit_bindings(child, visit);
            }
        }
    }
}

/// A loaded/imported workspace is a replacement, never a reference to live objects.
pub fn rekey_import(data: &mut WorkspaceData) {
    use std::collections::HashMap;
    let projects: HashMap<_, _> = data
        .projects
        .iter()
        .map(|p| (p.id.clone(), uuid::Uuid::new_v4().to_string()))
        .collect();
    let folders: HashMap<_, _> = data
        .folders
        .iter()
        .map(|f| (f.id.clone(), uuid::Uuid::new_v4().to_string()))
        .collect();
    let missions: HashMap<_, _> = data
        .missions
        .iter()
        .map(|m| (m.id.clone(), uuid::Uuid::new_v4().to_string()))
        .collect();
    let remap = |ids: &mut Vec<String>, map: &HashMap<String, String>| {
        *ids = ids.iter().filter_map(|id| map.get(id).cloned()).collect();
    };
    for project in &mut data.projects {
        if let Some(id) = projects.get(&project.id) {
            project.id = id.clone();
        }
        remap(&mut project.worktree_ids, &projects);
        if let Some(info) = &mut project.worktree_info {
            if let Some(id) = projects.get(&info.parent_project_id) {
                info.parent_project_id = id.clone();
            } else {
                project.worktree_info = None;
            }
        }
        if let Some(layout) = &mut project.layout {
            visit_bindings(layout, &mut |binding| {
                *binding = binding.as_ref().and_then(|id| missions.get(id).cloned());
            });
        }
    }
    for folder in &mut data.folders {
        if let Some(id) = folders.get(&folder.id) {
            folder.id = id.clone();
        }
        remap(&mut folder.project_ids, &projects);
    }
    data.project_order = data
        .project_order
        .iter()
        .filter_map(|id| projects.get(id).or_else(|| folders.get(id)).cloned())
        .collect();
    for mission in &mut data.missions {
        if let Some(id) = missions.get(&mission.id) {
            mission.id = id.clone();
        }
        mission.home_project_id = mission.home_project_id.as_ref().map(|id| {
            projects
                .get(id)
                .cloned()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        });
        remap(&mut mission.repository_ids, &projects);
        remap(&mut mission.worktree_ids, &projects);
        for pr in &mut mission.pull_requests {
            pr.available = false;
            pr.source_project_ids.clear();
        }
    }
    for window in std::iter::once(&mut data.main_window).chain(&mut data.extra_windows) {
        window.hidden_project_ids = window
            .hidden_project_ids
            .iter()
            .filter_map(|id| projects.get(id).cloned())
            .collect();
        window.project_widths = window
            .project_widths
            .iter()
            .filter_map(|(id, value)| projects.get(id).map(|id| (id.clone(), *value)))
            .collect();
        window.folder_collapsed = window
            .folder_collapsed
            .iter()
            .filter_map(|(id, value)| folders.get(id).map(|id| (id.clone(), *value)))
            .collect();
        window.folder_filter = window
            .folder_filter
            .as_ref()
            .and_then(|id| folders.get(id).cloned());
    }
    data.service_panel_heights = data
        .service_panel_heights
        .iter()
        .filter_map(|(id, value)| projects.get(id).map(|id| (id.clone(), *value)))
        .collect();
    data.hook_panel_heights = data
        .hook_panel_heights
        .iter()
        .filter_map(|(id, value)| projects.get(id).map(|id| (id.clone(), *value)))
        .collect();
    data.attention.restart();
    for observation in &mut data.attention.observations {
        if let Some(id) = projects.get(&observation.source.project_id) {
            observation.source.project_id = id.clone();
        }
        observation.source.attachment_id = uuid::Uuid::new_v4().to_string();
        for episode in [&mut observation.input, &mut observation.completion]
            .into_iter()
            .flatten()
        {
            episode.id = uuid::Uuid::new_v4().to_string();
            if let Some(id) = projects.get(&episode.source.project_id) {
                episode.source.project_id = id.clone();
            }
            episode.source.attachment_id = observation.source.attachment_id.clone();
        }
    }
    data.remote_work_overviews.clear();
}

pub fn work_overview(data: &WorkspaceData) -> WorkOverview {
    let mut overview = WorkOverview {
        missions: data.missions.clone(),
        attention: data.attention.episodes(),
        lost_transitions: data.attention.lost_transitions,
        ..Default::default()
    };
    for project in data.projects.iter().filter(|p| p.connection_id.is_none()) {
        for (terminal_id, session) in &project.agent_sessions {
            overview.conversations.push(ConversationAttachment {
                project_id: project.id.clone(),
                terminal_id: terminal_id.clone(),
                conversation: ConversationId::from(session),
            });
        }
        if let Some(layout) = &project.layout {
            for terminal_id in layout.collect_terminal_ids() {
                if let Some(mission_id) = terminal_mission(data, &project.id, &terminal_id) {
                    overview.terminal_bindings.push(TerminalMissionBinding {
                        project_id: project.id.clone(),
                        terminal_id,
                        mission_id: mission_id.into(),
                    });
                }
            }
        }
    }
    overview
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::agent_session::AgentSession;

    struct Cx;
    impl WorkspaceCx for Cx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<okena_hooks::HookRunner> {
            None
        }
        fn hook_monitor(&self) -> Option<okena_hooks::HookMonitor> {
            None
        }
    }

    #[test]
    fn pr_identity_deduplicates_sources_and_failure_preserves_last_observation() {
        let mut data = fixture();
        let id = create(
            &mut data,
            Some(MissionMember::Repository {
                project_id: "p".into(),
            }),
        );
        apply_command(
            &mut data,
            MissionCommand::Attach {
                mission_id: id,
                member: MissionMember::Worktree {
                    project_id: "w".into(),
                },
            },
        )
        .unwrap();
        let mut workspace = Workspace::new(data);
        let mut cx = Cx;
        let pr = pr_snapshot();
        workspace
            .record_mission_pr("p", Some(fetched(pr.clone())), &mut cx)
            .unwrap();
        workspace
            .record_mission_pr("w", Some(fetched(pr.clone())), &mut cx)
            .unwrap();
        assert_eq!(workspace.data.missions[0].pull_requests.len(), 1);
        workspace.record_mission_pr("p", None, &mut cx).unwrap();
        assert!(workspace.data.missions[0].pull_requests[0].available);
        workspace.record_mission_pr("w", None, &mut cx).unwrap();
        let retained = &workspace.data.missions[0].pull_requests[0];
        assert!(!retained.available);
        assert_eq!(retained.observed_at, 100);
        assert_eq!(retained.info.state, okena_core::api::PrState::Open);
        let mut other = pr.clone();
        other.identity.repository = "owner/other".into();
        workspace
            .record_mission_pr("w", Some(fetched(other)), &mut cx)
            .unwrap();
        let mut host = pr;
        host.identity.host = "git.example.com".into();
        workspace
            .record_mission_pr("p", Some(fetched(host)), &mut cx)
            .unwrap();
        assert_eq!(workspace.data.missions[0].pull_requests.len(), 3);
        workspace.data.projects.clear();
        workspace.notify_data(&mut cx);
        assert!(
            workspace.data.missions[0]
                .pull_requests
                .iter()
                .all(|pr| !pr.available)
        );
        assert_eq!(
            workspace.data.missions[0].lifecycle,
            MissionLifecycle::Active
        );
    }

    fn fetched(snapshot: MissionPullRequest) -> MissionPrObservation {
        MissionPrObservation {
            snapshot,
            pr_fetched: true,
            ci_fetched: true,
        }
    }

    #[test]
    fn pr_component_freshness_preserves_interleaved_updates_and_partial_failure() {
        let mut data = fixture();
        let mission = create(
            &mut data,
            Some(MissionMember::Repository {
                project_id: "p".into(),
            }),
        );
        apply_command(
            &mut data,
            MissionCommand::Attach {
                mission_id: mission,
                member: MissionMember::Worktree {
                    project_id: "w".into(),
                },
            },
        )
        .unwrap();
        let mut ws = Workspace::new(data);
        let mut cached = pr_snapshot();
        cached.ci = Some(okena_core::api::CiCheckSummary {
            status: okena_core::api::CiStatus::Success,
            passed: 1,
            failed: 0,
            pending: 0,
            total: 1,
            checks: Vec::new(),
        });
        ws.record_mission_pr("w", Some(fetched(cached.clone())), &mut Cx)
            .unwrap();
        let mut merged = cached.clone();
        merged.info.state = okena_core::api::PrState::Merged;
        merged.observed_at = 200;
        ws.record_mission_pr(
            "p",
            Some(MissionPrObservation {
                snapshot: merged.clone(),
                pr_fetched: true,
                ci_fetched: false,
            }),
            &mut Cx,
        )
        .unwrap();
        cached.observed_at = 300;
        ws.record_mission_pr(
            "w",
            Some(MissionPrObservation {
                snapshot: cached.clone(),
                pr_fetched: false,
                ci_fetched: false,
            }),
            &mut Cx,
        )
        .unwrap();
        let retained = &ws.data.missions[0].pull_requests[0];
        assert_eq!(retained.info.state, okena_core::api::PrState::Merged);
        assert_eq!(retained.observed_at, 200);
        assert_eq!(retained.ci, cached.ci);
        merged.available = false;
        merged.observed_at = 400;
        merged.ci = None;
        ws.record_mission_pr(
            "p",
            Some(MissionPrObservation {
                snapshot: merged.clone(),
                pr_fetched: true,
                ci_fetched: false,
            }),
            &mut Cx,
        )
        .unwrap();
        let retained = &ws.data.missions[0].pull_requests[0];
        assert_eq!(retained.source_project_ids, vec!["w"]);
        assert!(retained.available);
        assert_eq!(retained.ci, cached.ci);
        ws.record_mission_pr("w", None, &mut Cx).unwrap();
        ws.record_mission_pr(
            "p",
            Some(MissionPrObservation {
                snapshot: merged,
                pr_fetched: true,
                ci_fetched: false,
            }),
            &mut Cx,
        )
        .unwrap();
        let retained = &ws.data.missions[0].pull_requests[0];
        assert!(!retained.available);
        assert!(retained.source_project_ids.is_empty());
        assert_eq!(retained.info.state, okena_core::api::PrState::Merged);
        assert_eq!(retained.observed_at, 400);
        assert_eq!(retained.ci, cached.ci);
        cached.ci = None;
        cached.observed_at = 500;
        ws.record_mission_pr(
            "w",
            Some(MissionPrObservation {
                snapshot: cached,
                pr_fetched: false,
                ci_fetched: true,
            }),
            &mut Cx,
        )
        .unwrap();
        let retained = &ws.data.missions[0].pull_requests[0];
        assert!(retained.ci.is_none());
        assert_eq!(retained.info.state, okena_core::api::PrState::Merged);
        assert_eq!(retained.observed_at, 500);
    }

    fn pr_snapshot() -> MissionPullRequest {
        MissionPullRequest {
            identity: okena_core::mission::PullRequestIdentity {
                host: "github.com".into(),
                repository: "owner/repo".into(),
                number: 1,
            },
            info: okena_core::api::PrInfo {
                url: "https://github.com/owner/repo/pull/1".into(),
                state: okena_core::api::PrState::Open,
                number: 1,
                base: Some("main".into()),
            },
            ci: None,
            observed_at: 100,
            available: true,
            source_project_ids: Vec::new(),
        }
    }

    fn fixture() -> WorkspaceData {
        serde_json::from_value(serde_json::json!({
            "projects": [
                {"id":"p", "name":"Repo", "path":"/repo", "layout":{"type":"terminal", "terminal_id":"t"}},
                {"id":"w", "name":"Worktree", "path":"/worktree", "layout":{"type":"terminal", "terminal_id":"tw"}, "worktree_info":{"parent_project_id":"p"}}
            ], "project_order":["p", "w"]
        })).unwrap()
    }

    fn create(data: &mut WorkspaceData, member: Option<MissionMember>) -> String {
        apply_command(
            data,
            MissionCommand::Create {
                title: "Implement".into(),
                goal: None,
                home_project_id: Some("p".into()),
                member,
            },
        )
        .unwrap()
    }

    fn session(n: u64) -> AgentSession {
        AgentSession {
            agent: "claude-code".into(),
            session_id: format!("00000000-0000-0000-0000-{n:012x}"),
            transcript_path: Some(
                std::env::temp_dir()
                    .join("private-transcript.jsonl")
                    .to_string_lossy()
                    .into_owned(),
            ),
        }
    }

    #[test]
    fn conversation_detach_then_terminal_detach_excludes_the_next_conversation() {
        let mut data = fixture();
        let mission = create(
            &mut data,
            Some(MissionMember::Worktree {
                project_id: "w".into(),
            }),
        );
        let mut ws = Workspace::new(data);
        ws.set_agent_session("w", "tw", session(1), &mut Cx);
        ws.execute_mission(
            MissionCommand::Detach {
                mission_id: mission.clone(),
                member: MissionMember::Conversation {
                    conversation: (&session(1)).into(),
                },
            },
            &mut Cx,
        )
        .unwrap();
        assert!(matches!(
            ws.project("w").unwrap().layout.as_ref().unwrap(),
            LayoutNode::Terminal {
                mission_excluded: false,
                ..
            }
        ));
        ws.execute_mission(
            MissionCommand::Detach {
                mission_id: mission,
                member: MissionMember::Terminal {
                    project_id: "w".into(),
                    terminal_id: "tw".into(),
                },
            },
            &mut Cx,
        )
        .unwrap();
        ws.set_agent_session("w", "tw", session(2), &mut Cx);
        ws.record_attention(
            okena_core::attention::AttentionSource {
                project_id: "w".into(),
                terminal_id: "tw".into(),
                attachment_id: "boot".into(),
                generation: 1,
                conversation: Some((&session(2)).into()),
            },
            Some(&okena_core::agent_status::AgentStatus::new(
                okena_core::agent_status::AgentLifecycle::Working,
            )),
            1,
            &mut Cx,
        );
        assert_eq!(terminal_mission(&ws.data, "w", "tw"), None);
        assert_eq!(conversation_mission(&ws.data, &(&session(2)).into()), None);
        assert!(matches!(
            ws.project("w").unwrap().layout.as_ref().unwrap(),
            LayoutNode::Terminal {
                mission_excluded: true,
                ..
            }
        ));
    }

    #[test]
    fn excluded_leaf_stays_out_when_an_assigned_conversation_resumes() {
        let mut data = fixture();
        let default = create(
            &mut data,
            Some(MissionMember::Worktree {
                project_id: "w".into(),
            }),
        );
        let terminal = MissionMember::Terminal {
            project_id: "w".into(),
            terminal_id: "tw".into(),
        };
        apply_command(
            &mut data,
            MissionCommand::Detach {
                mission_id: default.clone(),
                member: terminal.clone(),
            },
        )
        .unwrap();
        data.agent_session_history.record(session(1));
        let primary = create(
            &mut data,
            Some(MissionMember::Conversation {
                conversation: (&session(1)).into(),
            }),
        );
        let mut ws = Workspace::new(data);
        ws.set_agent_session("w", "tw", session(1), &mut Cx);
        assert_eq!(terminal_mission(&ws.data, "w", "tw"), None);
        assert_eq!(
            conversation_mission(&ws.data, &(&session(1)).into()),
            Some(primary.as_str())
        );
        assert!(ws.data.mission_excluded_conversations.is_empty());
        ws.execute_mission(
            MissionCommand::Detach {
                mission_id: default.clone(),
                member: terminal.clone(),
            },
            &mut Cx,
        )
        .unwrap();
        assert_eq!(
            conversation_mission(&ws.data, &(&session(1)).into()),
            Some(primary.as_str())
        );
        assert!(
            ws.execute_mission(
                MissionCommand::Attach {
                    mission_id: default.clone(),
                    member: terminal.clone()
                },
                &mut Cx
            )
            .is_err()
        );
        assert_eq!(terminal_mission(&ws.data, "w", "tw"), None);
        ws.execute_mission(
            MissionCommand::Attach {
                mission_id: primary.clone(),
                member: terminal.clone(),
            },
            &mut Cx,
        )
        .unwrap();
        assert_eq!(
            terminal_mission(&ws.data, "w", "tw"),
            Some(primary.as_str())
        );
        ws.execute_mission(
            MissionCommand::Move {
                mission_id: default.clone(),
                member: terminal,
            },
            &mut Cx,
        )
        .unwrap();
        assert_eq!(
            terminal_mission(&ws.data, "w", "tw"),
            Some(default.as_str())
        );
    }

    #[test]
    fn moving_into_linked_worktree_records_history_before_any_new_report() {
        for into_tabs in [false, true] {
            let mut data = fixture();
            let mission = create(
                &mut data,
                Some(MissionMember::Worktree {
                    project_id: "w".into(),
                }),
            );
            if into_tabs {
                data.projects[1].layout = Some(LayoutNode::Tabs {
                    children: vec![data.projects[1].layout.take().unwrap()],
                    active_tab: 0,
                });
            }
            let mut ws = Workspace::new(data);
            ws.set_agent_session("p", "t", session(1), &mut Cx);
            assert!(conversation_mission(&ws.data, &(&session(1)).into()).is_none());
            if into_tabs {
                ws.move_terminal_to_tab_group(
                    &mut crate::focus::FocusManager::default(),
                    "p",
                    "t",
                    "w",
                    &[],
                    None,
                    &mut Cx,
                );
            } else {
                ws.move_pane(
                    &mut crate::focus::FocusManager::default(),
                    "p",
                    "t",
                    "w",
                    "tw",
                    crate::state::DropZone::Right,
                    &mut Cx,
                );
            }
            assert_eq!(
                conversation_mission(&ws.data, &(&session(1)).into()),
                Some(mission.as_str())
            );
            let path = ws
                .project("w")
                .unwrap()
                .layout
                .as_ref()
                .unwrap()
                .find_terminal_path("t")
                .unwrap();
            ws.close_terminal("w", &path, &mut Cx);
            assert!(!ws.project("w").unwrap().agent_sessions.contains_key("t"));
            assert_eq!(
                conversation_mission(&ws.data, &(&session(1)).into()),
                Some(mission.as_str())
            );
        }
    }

    #[test]
    fn two_repositories_keep_both_conversations_after_implementer_closes() {
        let mut data = fixture();
        let mut other = data.projects[0].clone();
        other.id = "other".into();
        other.path = "/other".into();
        other
            .layout
            .as_mut()
            .unwrap()
            .replace_terminal_id("t", "review");
        data.projects.push(other);
        let mission = create(
            &mut data,
            Some(MissionMember::Repository {
                project_id: "p".into(),
            }),
        );
        apply_command(
            &mut data,
            MissionCommand::Attach {
                mission_id: mission.clone(),
                member: MissionMember::Repository {
                    project_id: "other".into(),
                },
            },
        )
        .unwrap();
        let mut ws = Workspace::new(data);
        for (project_id, terminal_id, n) in [("p", "t", 1), ("other", "review", 2)] {
            ws.set_agent_session(project_id, terminal_id, session(n), &mut Cx);
            ws.execute_mission(
                MissionCommand::Attach {
                    mission_id: mission.clone(),
                    member: MissionMember::Terminal {
                        project_id: project_id.into(),
                        terminal_id: terminal_id.into(),
                    },
                },
                &mut Cx,
            )
            .unwrap();
        }
        ws.close_terminal("p", &[], &mut Cx);
        assert_eq!(ws.data.missions[0].repository_ids.len(), 2);
        assert_eq!(ws.data.missions[0].conversations.len(), 2);
        assert_eq!(ws.data.agent_session_history.sessions().len(), 2);
        assert_eq!(
            terminal_mission(&ws.data, "other", "review"),
            Some(mission.as_str())
        );
        assert!(ws.project("p").unwrap().agent_sessions.is_empty());
    }

    #[test]
    fn ordered_transitions_promote_each_conversation_and_respect_exclusions() {
        let mut data = fixture();
        let mission = create(
            &mut data,
            Some(MissionMember::Worktree {
                project_id: "w".into(),
            }),
        );
        data.mission_excluded_conversations
            .push((&session(3)).into());
        let mut ws = Workspace::new(data);
        for n in 1..=3 {
            ws.record_attention(
                okena_core::attention::AttentionSource {
                    project_id: "w".into(),
                    terminal_id: "tw".into(),
                    attachment_id: "boot".into(),
                    generation: 1,
                    conversation: Some((&session(n)).into()),
                },
                None,
                n,
                &mut Cx,
            );
        }
        assert_eq!(
            conversation_mission(&ws.data, &(&session(1)).into()),
            Some(mission.as_str())
        );
        assert_eq!(
            conversation_mission(&ws.data, &(&session(2)).into()),
            Some(mission.as_str())
        );
        assert_eq!(conversation_mission(&ws.data, &(&session(3)).into()), None);
    }

    #[test]
    fn cross_project_moves_preserve_attention_and_survive_source_project_removal() {
        for into_tabs in [false, true] {
            let mut data = fixture();
            data.projects[1].worktree_info = None;
            let source = okena_core::attention::AttentionSource {
                project_id: "p".into(),
                terminal_id: "t".into(),
                attachment_id: "boot".into(),
                generation: 1,
                conversation: Some((&session(1)).into()),
            };
            for (time, lifecycle) in [
                (1, okena_core::agent_status::AgentLifecycle::Done),
                (2, okena_core::agent_status::AgentLifecycle::Blocked),
            ] {
                data.attention.record(
                    source.clone(),
                    Some(&okena_core::agent_status::AgentStatus::new(lifecycle)),
                    time,
                    uuid::Uuid::new_v4().to_string(),
                );
            }
            let mut expected = data.attention.clone();
            expected.observations[0].source.project_id = "w".into();
            expected.observations[0]
                .input
                .as_mut()
                .unwrap()
                .source
                .project_id = "w".into();
            expected.observations[0]
                .completion
                .as_mut()
                .unwrap()
                .source
                .project_id = "w".into();
            if into_tabs {
                data.projects[1].layout = Some(LayoutNode::Tabs {
                    children: vec![data.projects[1].layout.take().unwrap()],
                    active_tab: 0,
                });
            }
            let mut ws = Workspace::new(data);
            if into_tabs {
                ws.move_terminal_to_tab_group(
                    &mut crate::focus::FocusManager::default(),
                    "p",
                    "t",
                    "w",
                    &[],
                    None,
                    &mut Cx,
                );
            } else {
                ws.move_pane(
                    &mut crate::focus::FocusManager::default(),
                    "p",
                    "t",
                    "w",
                    "tw",
                    crate::state::DropZone::Right,
                    &mut Cx,
                );
            }
            assert_eq!(ws.data.attention, expected);
            ws.delete_project(
                &mut crate::focus::FocusManager::default(),
                "p",
                &Default::default(),
                &mut Cx,
            );
            assert_eq!(ws.data.attention, expected);
            assert!(
                work_overview(&ws.data)
                    .attention
                    .iter()
                    .all(|e| e.available && e.source.project_id == "w")
            );
        }
    }

    #[test]
    fn anonymous_opt_out_survives_restore_import_and_does_not_spread_to_siblings() {
        let mut data = fixture();
        let mission = create(
            &mut data,
            Some(MissionMember::Worktree {
                project_id: "w".into(),
            }),
        );
        let member = MissionMember::Terminal {
            project_id: "w".into(),
            terminal_id: "tw".into(),
        };
        apply_command(
            &mut data,
            MissionCommand::Detach {
                mission_id: mission,
                member,
            },
        )
        .unwrap();
        assert_eq!(terminal_mission(&data, "w", "tw"), None);
        let mut data: WorkspaceData =
            serde_json::from_str(&serde_json::to_string(&data).unwrap()).unwrap();
        crate::persistence::validate_workspace_data(
            &mut data,
            true,
            okena_terminal::session_backend::SessionBackend::None,
        );
        rekey_import(&mut data);
        let project_id = data.projects[1].id.clone();
        let mission_id = data.missions[0].id.clone();
        let leaf = data.projects[1].layout.as_mut().unwrap();
        assert!(matches!(
            leaf,
            LayoutNode::Terminal {
                mission_excluded: true,
                terminal_id: None,
                ..
            }
        ));
        assert!(matches!(
            leaf.clone_structure(),
            LayoutNode::Terminal {
                mission_excluded: false,
                ..
            }
        ));
        if let LayoutNode::Terminal { terminal_id, .. } = leaf {
            *terminal_id = Some("restored".into());
        }
        let mut ws = Workspace::new(data);
        ws.split_terminal(
            &mut crate::focus::FocusManager::default(),
            &project_id,
            &[],
            okena_layout::SplitDirection::Horizontal,
            &mut Cx,
        );
        let layout = ws.data.projects[1].layout.as_mut().unwrap();
        if let Some(LayoutNode::Terminal {
            terminal_id,
            mission_excluded,
            ..
        }) = layout.get_at_path_mut(&[1])
        {
            assert!(!*mission_excluded);
            *terminal_id = Some("sibling".into());
        } else {
            panic!("new sibling expected");
        }
        assert_eq!(
            terminal_mission(&ws.data, &project_id, "sibling"),
            Some(mission_id.as_str())
        );
        ws.set_agent_session(&project_id, "restored", session(1), &mut Cx);
        assert_eq!(terminal_mission(&ws.data, &project_id, "restored"), None);
        assert!(
            ws.data
                .mission_excluded_conversations
                .contains(&(&session(1)).into())
        );
        ws.set_agent_session(&project_id, "sibling", session(2), &mut Cx);
        assert_eq!(
            terminal_mission(&ws.data, &project_id, "sibling"),
            Some(mission_id.as_str())
        );
        ws.execute_mission(
            MissionCommand::Attach {
                mission_id: mission_id.clone(),
                member: MissionMember::Terminal {
                    project_id: project_id.clone(),
                    terminal_id: "restored".into(),
                },
            },
            &mut Cx,
        )
        .unwrap();
        assert_eq!(
            terminal_mission(&ws.data, &project_id, "restored"),
            Some(mission_id.as_str())
        );
        assert!(ws.data.mission_excluded_conversations.is_empty());
    }

    #[test]
    fn conversation_detach_survives_reports_resume_and_import_until_explicit_move() {
        let mut data = fixture();
        let mission = create(
            &mut data,
            Some(MissionMember::Worktree {
                project_id: "w".into(),
            }),
        );
        let mut ws = Workspace::new(data);
        ws.set_agent_session("w", "tw", session(1), &mut Cx);
        let member = MissionMember::Conversation {
            conversation: (&session(1)).into(),
        };
        ws.execute_mission(
            MissionCommand::Detach {
                mission_id: mission.clone(),
                member: member.clone(),
            },
            &mut Cx,
        )
        .unwrap();
        ws.set_agent_session("w", "tw", session(1), &mut Cx);
        assert_eq!(terminal_mission(&ws.data, "w", "tw"), None);
        ws.set_agent_session("p", "t", session(1), &mut Cx);
        assert_eq!(terminal_mission(&ws.data, "p", "t"), None);
        let mut restored: WorkspaceData =
            serde_json::from_str(&serde_json::to_string(&ws.data).unwrap()).unwrap();
        crate::persistence::validate_workspace_data(
            &mut restored,
            false,
            okena_terminal::session_backend::SessionBackend::None,
        );
        rekey_import(&mut restored);
        let worktree = restored.projects[1].id.clone();
        assert_eq!(terminal_mission(&restored, &worktree, "tw"), None);
        assert!(restored.missions[0].conversations.is_empty());
        let target = apply_command(
            &mut restored,
            MissionCommand::Create {
                title: "Other".into(),
                goal: None,
                home_project_id: None,
                member: None,
            },
        )
        .unwrap();
        apply_command(
            &mut restored,
            MissionCommand::Move {
                mission_id: target.clone(),
                member,
            },
        )
        .unwrap();
        assert!(restored.mission_excluded_conversations.is_empty());
        assert_eq!(
            terminal_mission(&restored, &worktree, "tw"),
            Some(target.as_str())
        );
        assert!(restored.missions[0].conversations.is_empty());
    }

    #[test]
    fn detach_from_wrong_mission_does_not_mutate_membership() {
        let mut data = fixture();
        let owner = create(
            &mut data,
            Some(MissionMember::Worktree {
                project_id: "w".into(),
            }),
        );
        let other = create(&mut data, None);
        assert!(
            apply_command(
                &mut data,
                MissionCommand::Detach {
                    mission_id: other,
                    member: MissionMember::Terminal {
                        project_id: "w".into(),
                        terminal_id: "tw".into()
                    }
                }
            )
            .is_err()
        );
        assert_eq!(terminal_mission(&data, "w", "tw"), Some(owner.as_str()));
    }

    #[test]
    fn old_files_defaults_and_optional_repository_membership() {
        let mut data = fixture();
        assert!(data.missions.is_empty());
        assert!(data.attention.episodes().is_empty());
        let id = create(
            &mut data,
            Some(MissionMember::Repository {
                project_id: "p".into(),
            }),
        );
        assert!(terminal_mission(&data, "p", "t").is_none());
        apply_command(
            &mut data,
            MissionCommand::Attach {
                mission_id: id.clone(),
                member: MissionMember::Worktree {
                    project_id: "w".into(),
                },
            },
        )
        .unwrap();
        assert_eq!(terminal_mission(&data, "w", "tw"), Some(id.as_str()));
    }

    #[test]
    fn assignment_conflicts_require_move_and_history_survives_deletion() {
        let mut data = fixture();
        let session = session(1);
        data.agent_session_history.record(session.clone());
        data.projects[0]
            .agent_sessions
            .insert("t".into(), session.clone());
        let member = MissionMember::Terminal {
            project_id: "p".into(),
            terminal_id: "t".into(),
        };
        let a = create(&mut data, Some(member.clone()));
        let b = create(&mut data, None);
        assert!(
            apply_command(
                &mut data,
                MissionCommand::Attach {
                    mission_id: b.clone(),
                    member: member.clone()
                }
            )
            .is_err()
        );
        apply_command(
            &mut data,
            MissionCommand::Move {
                mission_id: b.clone(),
                member,
            },
        )
        .unwrap();
        assert_eq!(
            conversation_mission(&data, &(&session).into()),
            Some(b.as_str())
        );
        assert!(
            data.missions
                .iter()
                .find(|m| m.id == a)
                .unwrap()
                .conversations
                .is_empty()
        );
        data.projects.clear();
        reconcile_membership(&mut data);
        assert_eq!(
            conversation_mission(&data, &(&session).into()),
            Some(b.as_str())
        );
        assert!(data.agent_session_history.sessions().contains(&session));
    }

    #[test]
    fn resumed_assignment_wins_over_worktree_default() {
        let mut data = fixture();
        let session = session(1);
        data.agent_session_history.record(session.clone());
        let a = create(
            &mut data,
            Some(MissionMember::Conversation {
                conversation: (&session).into(),
            }),
        );
        let b = create(
            &mut data,
            Some(MissionMember::Worktree {
                project_id: "w".into(),
            }),
        );
        assert!(!promote_conversation(
            &mut data,
            "w",
            "tw",
            (&session).into()
        ));
        data.projects[1].agent_sessions.insert("tw".into(), session);
        assert_eq!(terminal_mission(&data, "w", "tw"), Some(a.as_str()));
        assert!(
            data.missions
                .iter()
                .find(|m| m.id == b)
                .unwrap()
                .conversations
                .is_empty()
        );
    }

    #[test]
    fn splitting_keeps_binding_on_original_leaf_only() {
        let mut data = fixture();
        let mission = create(
            &mut data,
            Some(MissionMember::Terminal {
                project_id: "p".into(),
                terminal_id: "t".into(),
            }),
        );
        let mut workspace = Workspace::new(data);
        workspace.split_terminal(
            &mut crate::focus::FocusManager::default(),
            "p",
            &[],
            okena_layout::SplitDirection::Horizontal,
            &mut Cx,
        );
        let layout = workspace.project("p").unwrap().layout.as_ref().unwrap();
        let LayoutNode::Split { children, .. } = layout else {
            panic!("split expected")
        };
        assert!(
            matches!(&children[0], LayoutNode::Terminal { mission_id: Some(id), .. } if id == &mission)
        );
        assert!(matches!(
            &children[1],
            LayoutNode::Terminal {
                mission_id: None,
                ..
            }
        ));
        workspace.move_pane(
            &mut crate::focus::FocusManager::default(),
            "p",
            "t",
            "w",
            "tw",
            crate::state::DropZone::Right,
            &mut Cx,
        );
        assert_eq!(
            terminal_mission(&workspace.data, "w", "t"),
            Some(mission.as_str())
        );
    }

    #[test]
    fn restore_rekey_and_split_preserve_only_original_binding() {
        let mut data = fixture();
        let mission = create(
            &mut data,
            Some(MissionMember::Terminal {
                project_id: "p".into(),
                terminal_id: "t".into(),
            }),
        );
        let session = session(1);
        data.projects[0]
            .agent_sessions
            .insert("t".into(), session.clone());
        crate::persistence::validate_workspace_data(
            &mut data,
            true,
            okena_terminal::session_backend::SessionBackend::None,
        );
        let leaf = data.projects[0].layout.as_ref().unwrap();
        assert!(
            matches!(leaf, LayoutNode::Terminal { terminal_id: None, mission_id: Some(id), pending_agent_resume: Some(s), .. } if id == &mission && s == &session)
        );
        let sibling = leaf.clone_structure();
        assert!(matches!(
            sibling,
            LayoutNode::Terminal {
                mission_id: None,
                pending_agent_resume: None,
                ..
            }
        ));
        rekey_import(&mut data);
        let new_mission = &data.missions[0];
        assert_ne!(new_mission.id, mission);
        assert_ne!(data.projects[0].id, "p");
        assert_eq!(
            new_mission.home_project_id.as_deref(),
            Some(data.projects[0].id.as_str())
        );
        assert!(
            matches!(data.projects[0].layout.as_ref().unwrap(), LayoutNode::Terminal { mission_id: Some(id), .. } if id == &new_mission.id)
        );
    }

    #[test]
    fn cross_daemon_and_unknown_conversations_rejected_without_creation() {
        let mut data = fixture();
        data.projects[0].connection_id = Some("other".into());
        assert!(
            apply_command(
                &mut data,
                MissionCommand::Create {
                    title: "x".into(),
                    goal: None,
                    home_project_id: Some("p".into()),
                    member: None
                }
            )
            .is_err()
        );
        assert!(data.missions.is_empty());
        assert!(
            validate_member(
                &data,
                &MissionMember::Conversation {
                    conversation: (&session(1)).into()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn projection_has_no_transcript_paths_and_lifecycle_does_not_read_attention() {
        let mut data = fixture();
        let id = create(&mut data, None);
        data.projects[0]
            .agent_sessions
            .insert("t".into(), session(1));
        let source = okena_core::attention::AttentionSource {
            project_id: "p".into(),
            terminal_id: "t".into(),
            attachment_id: "boot".into(),
            generation: 1,
            conversation: Some((&session(1)).into()),
        };
        data.attention.record(
            source,
            Some(&okena_core::agent_status::AgentStatus::new(
                okena_core::agent_status::AgentLifecycle::Done,
            )),
            1,
            uuid::Uuid::new_v4().to_string(),
        );
        apply_command(
            &mut data,
            MissionCommand::SetLifecycle {
                mission_id: id,
                lifecycle: MissionLifecycle::Archived,
            },
        )
        .unwrap();
        let overview = work_overview(&data);
        assert_eq!(overview.attention.len(), 1);
        assert!(
            !serde_json::to_string(&overview)
                .unwrap()
                .contains("transcript")
        );
        let mut loaded: WorkspaceData =
            serde_json::from_str(&serde_json::to_string(&data).unwrap()).unwrap();
        crate::persistence::validate_workspace_data(
            &mut loaded,
            true,
            okena_terminal::session_backend::SessionBackend::None,
        );
        assert_eq!(loaded.attention.episodes().len(), 1);
        assert!(!loaded.attention.episodes()[0].available);
    }
}
