//! `okena status …`: the status board from the command line — a project's or
//! worktree's status, the board's columns, its master switch and the
//! per-window lanes view (a family's, or the overview's). See
//! `docs/reference/status-board.md`.
//!
//! Statuses live on the daemon's projects (`set_project_status`); columns and
//! the switch are app settings (`get_settings` / `set_settings`); lanes are
//! desktop presentation the server pushes on (`set_status_lanes`,
//! `set_overview_status_lanes`). Everything
//! that resolves a name to an id is pure and unit-tested at the bottom.

use crate::commands::{fetch_state, post_action, validate_color};
use crate::parser::OnOffArg;
use crate::{api_action, ensure_token, resolve};
use okena_core::api::{ApiProject, StateResponse};
use okena_core::theme::FolderColor;
use okena_core::types::BoardColumn;
use okena_workspace::settings::StatusBoardSettings;
use serde_json::{Value, json};

const BOARD_OFF_NOTE: &str = "Note: the status board is off, so the app shows no statuses or \
lanes. Turn it on with `okena status enable`.";

// ── Pure helpers ─────────────────────────────────────────────────────────────

/// The status board settings inside a `get_settings` reply. A daemon from
/// before the board has none and gets the defaults.
fn board_from_settings(settings: &Value) -> Result<StatusBoardSettings, String> {
    match settings.get("status_board") {
        None | Some(Value::Null) => Ok(StatusBoardSettings::default()),
        Some(board) => serde_json::from_value(board.clone())
            .map_err(|e| format!("Unreadable status_board settings: {e}")),
    }
}

/// `To do (todo), Review (review)`: the columns as a correction hint.
fn columns_hint(columns: &[BoardColumn]) -> String {
    if columns.is_empty() {
        return "(none — add one with `okena status column add <name>`)".to_string();
    }
    columns
        .iter()
        .map(|c| format!("{} ({})", c.name, c.id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve a column by exact id, then by case-insensitive name. Two columns
/// sharing a name are an ambiguity the caller resolves with an id.
fn resolve_column<'a>(columns: &'a [BoardColumn], filter: &str) -> Result<&'a BoardColumn, String> {
    if let Some(column) = columns.iter().find(|c| c.id == filter) {
        return Ok(column);
    }
    let wanted = filter.trim();
    let by_name: Vec<&BoardColumn> = columns
        .iter()
        .filter(|c| c.name.trim().eq_ignore_ascii_case(wanted))
        .collect();
    match by_name.as_slice() {
        [column] => Ok(column),
        [] => Err(format!(
            "Status column not found: {filter}\nColumns: {}",
            columns_hint(columns)
        )),
        many => Err(format!(
            "Column name '{filter}' is ambiguous — use an id: {}",
            many.iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// A column name the CLI accepts: not blank and not another column's name
/// (case-insensitive), so names stay usable as addresses. `except` is the
/// column being renamed.
fn validate_column_name(
    columns: &[BoardColumn],
    name: &str,
    except: Option<&str>,
) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A column name cannot be empty.".to_string());
    }
    if let Some(taken) = columns
        .iter()
        .find(|c| Some(c.id.as_str()) != except && c.name.trim().eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "A column named '{}' already exists ({}).",
            taken.name, taken.id
        ));
    }
    Ok(name.to_string())
}

fn parse_color(color: &str) -> Result<FolderColor, String> {
    let color = validate_color(color)?;
    serde_json::from_value(Value::String(color.clone()))
        .map_err(|_| format!("Invalid color: {color}"))
}

fn color_name(color: FolderColor) -> String {
    serde_json::to_value(color)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The project a command addresses: the filter, else the project of the
/// terminal the CLI runs in (`$OKENA_TERMINAL_ID`). Never the focused
/// project: a status set on the wrong worktree is easy to miss.
fn target_project<'a>(
    state: &'a StateResponse,
    filter: Option<&str>,
    own_terminal: Option<&str>,
) -> Result<&'a ApiProject, String> {
    if let Some(filter) = filter {
        return resolve::resolve_project(state, filter);
    }
    let terminal_id = own_terminal.ok_or(
        "No project given, and this is not an Okena terminal ($OKENA_TERMINAL_ID is not set). \
         Name the project or worktree.",
    )?;
    resolve::project_of_terminal(state, terminal_id).ok_or_else(|| {
        format!("No project given, and terminal {terminal_id} belongs to no project. Name one.")
    })
}

fn own_terminal_id() -> Option<String> {
    std::env::var("OKENA_TERMINAL_ID")
        .ok()
        .filter(|id| !id.is_empty())
}

/// A project and the column its status resolves to. `column` is `None` both
/// without a status and for a removed column's id (`project.status` tells
/// them apart).
struct StatusRow<'a> {
    project: &'a ApiProject,
    column: Option<&'a BoardColumn>,
}

/// `projects` in board order: the columns in their order, then everything
/// without a (still existing) column. Input order is kept inside each group,
/// like the sidebar and the lanes.
fn board_rows<'a>(projects: Vec<&'a ApiProject>, columns: &'a [BoardColumn]) -> Vec<StatusRow<'a>> {
    let position = |p: &ApiProject| {
        p.status
            .as_deref()
            .and_then(|s| columns.iter().position(|c| c.id == s))
    };
    let mut rows: Vec<(usize, StatusRow<'a>)> = projects
        .into_iter()
        .map(|project| {
            let slot = position(project);
            let row = StatusRow {
                project,
                column: slot.map(|i| &columns[i]),
            };
            (slot.unwrap_or(usize::MAX), row)
        })
        .collect();
    rows.sort_by_key(|(slot, _)| *slot);
    rows.into_iter().map(|(_, row)| row).collect()
}

/// `name<TAB>column name<TAB>column id`: both column fields empty without a
/// status, the name empty for a removed column's id.
fn row_line(row: &StatusRow) -> String {
    format!(
        "{}\t{}\t{}",
        row.project.name,
        row.column.map(|c| c.name.as_str()).unwrap_or(""),
        row.project.status.as_deref().unwrap_or("")
    )
}

fn row_json(row: &StatusRow) -> Value {
    json!({
        "id": row.project.id,
        "name": row.project.name,
        "path": row.project.path,
        "parent_id": row.project.worktree_info.as_ref().map(|wt| wt.parent_project_id.clone()),
        "status": row.project.status,
        "status_name": row.column.map(|c| c.name.clone()),
    })
}

fn column_json(column: &BoardColumn) -> Value {
    json!({
        "id": column.id,
        "name": column.name,
        "color": color_name(column.color),
    })
}

/// The column name for `okena ls`: the column's name, or the raw id with a
/// marker when the column was removed.
pub(crate) fn status_label(status: &str, board: Option<&StatusBoardSettings>) -> String {
    match board {
        Some(board) => match board.column(status) {
            Some(column) => column.name.clone(),
            None => format!("{status} (removed column)"),
        },
        None => status.to_string(),
    }
}

// ── I/O plumbing ─────────────────────────────────────────────────────────────

fn fetch_board(token: &str) -> Result<StatusBoardSettings, String> {
    let resp = api_action(token, &json!({ "action": "get_settings" }).to_string())?;
    let settings: Value =
        serde_json::from_str(&resp).map_err(|e| format!("Bad settings response: {e}"))?;
    board_from_settings(&settings)
}

/// The board as `okena ls` shows it, if the daemon answers.
pub(crate) fn try_fetch_board(token: &str) -> Option<StatusBoardSettings> {
    fetch_board(token).ok()
}

fn save_board_patch(token: &str, patch: Value) -> Result<(), String> {
    let body = json!({ "action": "set_settings", "patch": { "status_board": patch } });
    api_action(token, &body.to_string()).map(|_| ())
}

fn save_columns(token: &str, columns: &[BoardColumn]) -> Result<(), String> {
    save_board_patch(token, json!({ "columns": columns }))
}

/// Authenticate, then run `f`; print its error and exit 1 on failure. An
/// empty error was already reported (see [`posted`]).
fn run(f: impl FnOnce(&str) -> Result<(), String>) -> i32 {
    let result = ensure_token().and_then(|token| f(&token));
    match result {
        Ok(()) => 0,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}");
            }
            1
        }
    }
}

fn print_json(value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// `okena status ls [project] [--json]`
pub fn cli_status_ls(project: Option<&str>, json_mode: bool) -> i32 {
    run(|token| {
        let state = fetch_state(token)?;
        let board = fetch_board(token)?;
        let projects = match project {
            Some(filter) => {
                resolve::project_family(&state, resolve::resolve_project(&state, filter)?)
            }
            None => state.projects.iter().collect(),
        };
        let rows = board_rows(projects, &board.columns);
        if json_mode {
            return print_json(&json!({
                "enabled": board.enabled,
                "columns": board.columns.iter().map(column_json).collect::<Vec<_>>(),
                "no_status_color": color_name(board.no_status_color),
                "projects": rows.iter().map(row_json).collect::<Vec<_>>(),
            }));
        }
        for row in &rows {
            println!("{}", row_line(row));
        }
        Ok(())
    })
}

/// `okena status get [project] [--json]`
pub fn cli_status_get(project: Option<&str>, json_mode: bool) -> i32 {
    run(|token| {
        let state = fetch_state(token)?;
        let board = fetch_board(token)?;
        let terminal = own_terminal_id();
        let project = target_project(&state, project, terminal.as_deref())?;
        let row = StatusRow {
            project,
            column: project.status.as_deref().and_then(|s| board.column(s)),
        };
        if json_mode {
            return print_json(&row_json(&row));
        }
        if let Some(status) = &project.status {
            println!(
                "{}\t{status}",
                row.column.map(|c| c.name.as_str()).unwrap_or("")
            );
        }
        Ok(())
    })
}

/// `okena status set <column> [project]`
pub fn cli_status_set(column: &str, project: Option<&str>) -> i32 {
    run(|token| {
        let state = fetch_state(token)?;
        let board = fetch_board(token)?;
        let terminal = own_terminal_id();
        let project = target_project(&state, project, terminal.as_deref())?;
        let column = resolve_column(&board.columns, column)?;
        let body = json!({
            "action": "set_project_status",
            "project_id": project.id,
            "status": column.id,
        });
        posted(post_action(token, &body))?;
        if !board.enabled {
            eprintln!("{BOARD_OFF_NOTE}");
        }
        Ok(())
    })
}

/// `okena status clear [project]`
pub fn cli_status_clear(project: Option<&str>) -> i32 {
    run(|token| {
        let state = fetch_state(token)?;
        let terminal = own_terminal_id();
        let project = target_project(&state, project, terminal.as_deref())?;
        let body = json!({
            "action": "set_project_status",
            "project_id": project.id,
            "status": Value::Null,
        });
        posted(post_action(token, &body))
    })
}

/// `okena status lanes <on|off> [project | --overview] [--window]`
pub fn cli_status_lanes(
    state_arg: OnOffArg,
    project: Option<&str>,
    overview: bool,
    window: Option<&str>,
) -> i32 {
    run(|token| {
        let state = fetch_state(token)?;
        let window = window
            .map(|w| resolve::resolve_window(&state, w))
            .transpose()?;
        let on = state_arg == OnOffArg::On;
        let action = if overview {
            okena_core::api::ActionRequest::SetOverviewStatusLanes { on, window }
        } else {
            let terminal = own_terminal_id();
            let project = target_project(&state, project, terminal.as_deref())?;
            okena_core::api::ActionRequest::SetStatusLanes {
                project_id: project.id.clone(),
                on,
                window,
            }
        };
        let body = serde_json::to_value(action).map_err(|e| e.to_string())?;
        posted(post_action(token, &body))?;
        if fetch_board(token).is_ok_and(|board| !board.enabled) {
            eprintln!("{BOARD_OFF_NOTE}");
        }
        Ok(())
    })
}

/// `okena status enable|disable`
pub fn cli_status_enable(enabled: bool) -> i32 {
    run(|token| save_board_patch(token, json!({ "enabled": enabled })))
}

/// `okena status column ls [--json]`
pub fn cli_status_column_ls(json_mode: bool) -> i32 {
    run(|token| {
        let board = fetch_board(token)?;
        if json_mode {
            return print_json(&Value::Array(
                board.columns.iter().map(column_json).collect(),
            ));
        }
        for column in &board.columns {
            println!(
                "{}\t{}\t{}",
                column.id,
                column.name,
                color_name(column.color)
            );
        }
        Ok(())
    })
}

/// `okena status column add <name> [--color c]` — prints the new id.
pub fn cli_status_column_add(name: &str, color: Option<&str>) -> i32 {
    run(|token| {
        let color = color.map(parse_color).transpose()?;
        let mut board = fetch_board(token)?;
        let name = validate_column_name(&board.columns, name, None)?;
        let id = board.add_column(&name);
        if let Some(color) = color {
            board.set_column_color(&id, color);
        }
        save_columns(token, &board.columns)?;
        println!("{id}");
        Ok(())
    })
}

/// `okena status column rename <column> <name>`
pub fn cli_status_column_rename(column: &str, name: &str) -> i32 {
    edit_columns(column, |board, id| {
        let name = validate_column_name(&board.columns, name, Some(id))?;
        board.rename_column(id, name);
        Ok(())
    })
}

/// `okena status column color <column> <color>`
pub fn cli_status_column_color(column: &str, color: &str) -> i32 {
    edit_columns(column, |board, id| {
        board.set_column_color(id, parse_color(color)?);
        Ok(())
    })
}

/// `okena status column move <column> <position>` (1-based)
pub fn cli_status_column_move(column: &str, position: usize) -> i32 {
    edit_columns(column, |board, id| {
        let count = board.columns.len();
        if position == 0 || position > count {
            return Err(format!("Position must be between 1 and {count}."));
        }
        board.move_column(id, position - 1);
        Ok(())
    })
}

/// `okena status column rm <column>`
pub fn cli_status_column_rm(column: &str) -> i32 {
    run(|token| {
        let mut board = fetch_board(token)?;
        let id = resolve_column(&board.columns, column)?.id.clone();
        board.remove_column(&id);
        save_columns(token, &board.columns)?;
        // Like the settings editor, nothing is deleted on the user's behalf.
        let kept = fetch_state(token)
            .map(|state| {
                state
                    .projects
                    .iter()
                    .filter(|p| p.status.as_deref() == Some(id.as_str()))
                    .count()
            })
            .unwrap_or(0);
        if kept > 0 {
            eprintln!(
                "{kept} project(s) keep the status id {id} and show as No status until re-sorted."
            );
        }
        Ok(())
    })
}

/// `okena status column no-status [color]`: print the No status colour, or
/// set it.
pub fn cli_status_column_no_status(color: Option<&str>) -> i32 {
    run(|token| match color {
        None => {
            println!("{}", color_name(fetch_board(token)?.no_status_color));
            Ok(())
        }
        Some(color) => {
            let color = parse_color(color)?;
            save_board_patch(token, json!({ "no_status_color": color_name(color) }))
        }
    })
}

/// Fetch the board, resolve `column`, apply `edit` and save the columns.
fn edit_columns(
    column: &str,
    edit: impl FnOnce(&mut StatusBoardSettings, &str) -> Result<(), String>,
) -> i32 {
    run(|token| {
        let mut board = fetch_board(token)?;
        let id = resolve_column(&board.columns, column)?.id.clone();
        edit(&mut board, &id)?;
        save_columns(token, &board.columns)
    })
}

/// `post_action` already printed the failure; carry it out as an error with
/// nothing more to say.
fn posted(code: i32) -> Result<(), String> {
    if code == 0 {
        Ok(())
    } else {
        Err(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_core::api::ApiWorktreeMetadata;
    use okena_core::types::default_board_columns;

    fn project(id: &str, status: Option<&str>) -> ApiProject {
        serde_json::from_value(json!({
            "id": id,
            "name": id.to_uppercase(),
            "path": format!("/tmp/{id}"),
            "show_in_overview": true,
            "layout": null,
            "terminal_names": {},
            "status": status,
            "folder_color": "default",
            "services": [],
        }))
        .expect("minimal ApiProject")
    }

    fn state(projects: Vec<ApiProject>) -> StateResponse {
        serde_json::from_value(json!({
            "state_version": 1,
            "projects": serde_json::to_value(projects).unwrap(),
            "focused_project_id": null,
            "fullscreen_terminal": null,
        }))
        .expect("minimal StateResponse")
    }

    #[test]
    fn board_settings_default_when_the_daemon_has_none() {
        let board = board_from_settings(&json!({ "font_size": 14 })).unwrap();
        assert!(!board.enabled);
        assert_eq!(board.columns, default_board_columns());

        let board = board_from_settings(&json!({
            "status_board": {
                "enabled": true,
                "columns": [{ "id": "x1", "name": "Blocked", "color": "red" }],
            }
        }))
        .unwrap();
        assert!(board.enabled);
        assert_eq!(board.columns[0].color, FolderColor::Red);
        assert_eq!(board.no_status_color, FolderColor::Default);

        let board = board_from_settings(&json!({
            "status_board": { "no_status_color": "teal" }
        }))
        .unwrap();
        assert_eq!(board.no_status_color, FolderColor::Teal);
        assert!(board_from_settings(&json!({ "status_board": { "columns": 3 } })).is_err());
    }

    #[test]
    fn columns_resolve_by_id_then_name_and_report_ambiguity() {
        let mut columns = default_board_columns();
        assert_eq!(resolve_column(&columns, "review").unwrap().id, "review");
        assert_eq!(
            resolve_column(&columns, " in PROGRESS ").unwrap().id,
            "in-progress"
        );

        let miss = resolve_column(&columns, "blocked").unwrap_err();
        assert!(miss.contains("To do (todo)"), "{miss}");

        columns.push(BoardColumn::new("u1", "Review"));
        let ambiguous = resolve_column(&columns, "REVIEW").unwrap_err();
        assert!(ambiguous.contains("review, u1"), "{ambiguous}");
        // An id still wins over a name.
        assert_eq!(resolve_column(&columns, "u1").unwrap().name, "Review");
    }

    #[test]
    fn column_names_must_be_present_and_unique() {
        let columns = default_board_columns();
        assert_eq!(
            validate_column_name(&columns, "  Blocked ", None).unwrap(),
            "Blocked"
        );
        assert!(validate_column_name(&columns, "   ", None).is_err());
        assert!(validate_column_name(&columns, "to DO", None).is_err());
        // Renaming a column to its own name (another case) is fine.
        assert_eq!(
            validate_column_name(&columns, "To Do", Some("todo")).unwrap(),
            "To Do"
        );
        assert!(validate_column_name(&columns, "Done", Some("todo")).is_err());
    }

    #[test]
    fn colors_parse_like_project_colors() {
        assert_eq!(parse_color("Teal").unwrap(), FolderColor::Teal);
        assert!(parse_color("mauve").is_err());
        assert_eq!(color_name(FolderColor::Default), "default");
    }

    #[test]
    fn the_target_project_is_named_or_the_calling_terminals() {
        let mut own = project("own", None);
        own.terminal_names.insert("t-own".into(), "shell".into());
        let state = state(vec![project("other", None), own]);

        assert_eq!(
            target_project(&state, Some("other"), Some("t-own"))
                .unwrap()
                .id,
            "other"
        );
        assert_eq!(
            target_project(&state, None, Some("t-own")).unwrap().id,
            "own"
        );
        let no_env = target_project(&state, None, None)
            .map(|p| p.id.clone())
            .unwrap_err();
        assert!(no_env.contains("OKENA_TERMINAL_ID"), "{no_env}");
        assert!(target_project(&state, None, Some("stray")).is_err());
    }

    #[test]
    fn rows_follow_the_board_order_then_no_status() {
        let columns = default_board_columns();
        let projects = [
            project("a", None),
            project("b", Some("review")),
            project("c", Some("gone")),
            project("d", Some("todo")),
            project("e", Some("review")),
        ];
        let rows = board_rows(projects.iter().collect(), &columns);
        let lines: Vec<String> = rows.iter().map(row_line).collect();
        assert_eq!(
            lines,
            [
                "D\tTo do\ttodo",
                "B\tReview\treview",
                "E\tReview\treview",
                "A\t\t",
                "C\t\tgone",
            ]
        );
    }

    #[test]
    fn row_json_names_the_parent_and_the_column() {
        let columns = default_board_columns();
        let mut wt = project("wt", Some("done"));
        wt.worktree_info = Some(ApiWorktreeMetadata {
            parent_project_id: "p".into(),
            color_override: None,
            custom_name: false,
        });
        let row = StatusRow {
            project: &wt,
            column: columns.iter().find(|c| c.id == "done"),
        };
        let value = row_json(&row);
        assert_eq!(value["parent_id"], "p");
        assert_eq!(value["status"], "done");
        assert_eq!(value["status_name"], "Done");
    }

    #[test]
    fn ls_labels_a_removed_column() {
        let board = StatusBoardSettings::default();
        assert_eq!(status_label("review", Some(&board)), "Review");
        assert_eq!(status_label("gone", Some(&board)), "gone (removed column)");
        assert_eq!(status_label("review", None), "review");
    }
}
