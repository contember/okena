// Types matching crates/okena-core exactly

// ── REST types ──────────────────────────────────────────────────────────────

export interface HealthResponse {
  status: string;
  version: string;
  uptime_secs: number;
}

export interface StateResponse {
  state_version: number;
  projects: ApiProject[];
  focused_project_id: string | null;
  fullscreen_terminal: ApiFullscreen | null;
  project_order?: string[];
  folders?: ApiFolder[];
  windows?: ApiWindow[];
}

export interface ApiProject {
  id: string;
  name: string;
  path: string;
  show_in_overview: boolean;
  layout: ApiLayoutNode | null;
  terminal_names: Record<string, string>;
  git_status?: ApiGitStatus | null;
  folder_color?: string;
  services?: ApiServiceInfo[];
  worktree_info?: ApiWorktreeMetadata | null;
  worktree_ids?: string[];
  pinned?: boolean;
  last_activity_at?: number | null;
  default_shell?: ShellType | null;
  hook_terminals?: ApiHookTerminalEntry[];
  hooks?: ApiHooksConfig;
}

export interface ApiFolder {
  id: string;
  name: string;
  project_ids: string[];
  folder_color?: string;
}

export type PrState = "Open" | "Merged" | "Closed" | "Draft";

export interface PrInfo {
  url: string;
  state: PrState;
  number: number;
}

export type CiStatus = "Success" | "Failure" | "Pending";

export interface CiCheck {
  name: string;
  workflow?: string;
  status: CiStatus;
  is_skipped?: boolean;
  link?: string;
  description?: string;
  elapsed_ms?: number;
}

export interface CiCheckSummary {
  status: CiStatus;
  passed: number;
  failed: number;
  pending: number;
  total: number;
  checks?: CiCheck[];
}

export interface ApiGitStatus {
  branch: string | null;
  lines_added: number;
  lines_removed: number;
  pr_info?: PrInfo | null;
  ci_checks?: CiCheckSummary | null;
  ahead?: number | null;
  behind?: number | null;
  unpushed?: number | null;
  review_base?: string | null;
}

export interface FileDiffSummary {
  path: string;
  added: number;
  removed: number;
  is_new: boolean;
}

export interface DirectoryEntry {
  name: string;
  is_dir: boolean;
}

export type DiffMode =
  | "working_tree"
  | "staged"
  | { commit: string }
  | { branch_compare: { base: string; head: string } };

export type FolderColor =
  | "default"
  | "red"
  | "orange"
  | "yellow"
  | "lime"
  | "green"
  | "teal"
  | "cyan"
  | "blue"
  | "indigo"
  | "purple"
  | "pink";

export type ShellType =
  | { type: "Default" }
  | { type: "Custom"; path: string; args?: string[] }
  | { type: "Cmd" }
  | { type: "PowerShell"; core?: boolean }
  | { type: "Wsl"; distro?: string | null };

export interface ApiWindowBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ApiWindow {
  id: string;
  kind: string;
  active: boolean;
  focused_project_id?: string | null;
  focused_terminal_id?: string | null;
  fullscreen?: ApiFullscreen | null;
  visible_project_ids?: string[];
  folder_filter?: string | null;
  bounds?: ApiWindowBounds | null;
  sidebar_open?: boolean | null;
}

export interface ApiServiceInfo {
  name: string;
  status: string;
  terminal_id: string | null;
  ports?: number[];
  exit_code?: number | null;
  kind?: string;
  is_extra?: boolean;
}

export interface ApiWorktreeMetadata {
  parent_project_id: string;
  color_override?: FolderColor | null;
}

export type ApiHookTerminalStatus =
  | { state: "running" }
  | { state: "succeeded" }
  | { state: "failed"; exit_code: number };

export interface ApiHookTerminalEntry {
  terminal_id: string;
  label: string;
  status: ApiHookTerminalStatus;
  hook_type: string;
  command: string;
  cwd: string;
}

export interface ApiProjectHooks {
  on_open?: string | null;
  on_close?: string | null;
}

export interface ApiTerminalHooks {
  on_create?: string | null;
  on_close?: string | null;
  shell_wrapper?: string | null;
}

export interface ApiWorktreeHooks {
  on_create?: string | null;
  on_close?: string | null;
  pre_merge?: string | null;
  post_merge?: string | null;
  before_remove?: string | null;
  after_remove?: string | null;
  on_rebase_conflict?: string | null;
  on_dirty_close?: string | null;
}

export interface ApiHooksConfig {
  project?: ApiProjectHooks;
  terminal?: ApiTerminalHooks;
  worktree?: ApiWorktreeHooks;
}

export interface ApiToastAction {
  id: string;
  label: string;
  style: string;
}

export interface ApiToast {
  id: string;
  level: string;
  message: string;
  detail?: string | null;
  ttl_ms: number;
  actions?: ApiToastAction[];
}

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

// serde(tag = "type", rename_all = "lowercase")
export type ApiLayoutNode =
  | {
      type: "terminal";
      terminal_id: string | null;
      minimized: boolean;
      detached: boolean;
      cols?: number | null;
      rows?: number | null;
    }
  | { type: "split"; direction: SplitDirection; sizes: number[]; children: ApiLayoutNode[] }
  | { type: "tabs"; children: ApiLayoutNode[]; active_tab: number };

// serde(rename_all = "lowercase")
export type SplitDirection = "horizontal" | "vertical";

export interface ApiFullscreen {
  project_id: string;
  terminal_id: string;
}

// serde(tag = "action", rename_all = "snake_case")
export type ActionRequest =
  | { action: "send_text"; terminal_id: string; text: string }
  | { action: "run_command"; terminal_id: string; command: string }
  | { action: "send_special_key"; terminal_id: string; key: SpecialKey }
  | { action: "split_terminal"; project_id: string; path: number[]; direction: SplitDirection }
  | { action: "close_terminal"; project_id: string; terminal_id: string }
  | { action: "close_terminals"; project_id: string; terminal_ids: string[] }
  | { action: "undo_soft_close"; terminal_id: string }
  | { action: "close_terminal_now"; terminal_id: string }
  | { action: "focus_terminal"; project_id: string; terminal_id: string; window?: string | null }
  | { action: "read_content"; terminal_id: string }
  | { action: "export_buffer"; terminal_id: string }
  | { action: "resize"; terminal_id: string; cols: number; rows: number }
  | { action: "create_terminal"; project_id: string }
  | { action: "update_split_sizes"; project_id: string; path: number[]; sizes: number[] }
  | { action: "toggle_minimized"; project_id: string; terminal_id: string }
  | { action: "set_fullscreen"; project_id: string; terminal_id: string | null; window?: string | null }
  | { action: "rename_terminal"; project_id: string; terminal_id: string; name: string }
  | { action: "switch_terminal_shell"; project_id: string; terminal_id: string; shell: ShellType }
  | { action: "add_tab"; project_id: string; path: number[]; in_group: boolean }
  | { action: "set_active_tab"; project_id: string; path: number[]; index: number }
  | { action: "move_tab"; project_id: string; path: number[]; from_index: number; to_index: number }
  | {
      action: "move_terminal_to_tab_group";
      project_id: string;
      terminal_id: string;
      target_path: number[];
      position?: number | null;
      target_project_id?: string | null;
    }
  | {
      action: "move_pane_to";
      project_id: string;
      terminal_id: string;
      target_project_id: string;
      target_terminal_id: string;
      zone: string;
    }
  | { action: "git_status"; project_id: string }
  | { action: "git_diff_summary"; project_id: string }
  | { action: "git_diff"; project_id: string; mode?: DiffMode; ignore_whitespace?: boolean }
  | { action: "git_branches"; project_id: string }
  | { action: "git_file_contents"; project_id: string; file_path: string; mode?: DiffMode }
  | { action: "git_commit_graph"; project_id: string; count: number; branch?: string | null }
  | { action: "git_list_branches"; project_id: string }
  | { action: "git_list_worktrees"; project_id: string }
  | { action: "worktree_close_info"; project_id: string }
  | { action: "generate_worktree_branch_name"; project_id: string }
  | { action: "git_list_branches_classified"; project_id: string }
  | { action: "git_checkout_local_branch"; project_id: string; branch: string }
  | { action: "git_checkout_remote_branch"; project_id: string; remote_branch: string }
  | { action: "git_create_and_checkout_branch"; project_id: string; new_name: string; start_point?: string | null }
  | { action: "git_stage_file"; project_id: string; file_path: string }
  | { action: "git_unstage_file"; project_id: string; file_path: string }
  | { action: "git_discard_file"; project_id: string; file_path: string }
  | { action: "git_blame"; project_id: string; relative_path: string }
  | { action: "add_project"; name: string; path: string }
  | { action: "reorder_project_in_folder"; folder_id: string; project_id: string; new_index: number }
  | { action: "set_project_color"; project_id: string; color: FolderColor }
  | { action: "set_folder_color"; folder_id: string; color: FolderColor }
  | { action: "start_service"; project_id: string; service_name: string }
  | { action: "stop_service"; project_id: string; service_name: string }
  | { action: "restart_service"; project_id: string; service_name: string }
  | { action: "start_all_services"; project_id: string }
  | { action: "stop_all_services"; project_id: string }
  | { action: "reload_services"; project_id: string }
  | { action: "create_worktree"; project_id: string; branch: string; create_branch?: boolean }
  | { action: "add_discovered_worktree"; parent_project_id: string; worktree_path: string; branch: string }
  | { action: "rerun_hook"; project_id: string; terminal_id: string }
  | { action: "list_files"; project_id: string; show_ignored?: boolean }
  | { action: "list_directory"; project_id: string; relative_path?: string; show_ignored?: boolean }
  | { action: "read_file"; project_id: string; relative_path: string }
  | { action: "read_file_bytes"; project_id: string; relative_path: string }
  | { action: "file_size"; project_id: string; relative_path: string }
  | {
      action: "search_content";
      project_id: string;
      query: string;
      case_sensitive?: boolean;
      mode?: string;
      max_results?: number;
      file_glob?: string | null;
      context_lines?: number;
    }
  | { action: "rename_file"; project_id: string; relative_path: string; new_name: string }
  | { action: "delete_file"; project_id: string; relative_path: string }
  | { action: "create_file"; project_id: string; relative_path: string }
  | { action: "create_directory"; project_id: string; relative_path: string }
  | { action: "rename_project"; project_id: string; name: string }
  | { action: "update_project_hooks"; project_id: string; hooks: ApiHooksConfig }
  | { action: "rename_project_directory"; project_id: string; new_name: string }
  | { action: "delete_project"; project_id: string }
  | { action: "set_project_show_in_overview"; project_id: string; show: boolean; window?: string | null }
  | { action: "remove_worktree_project"; project_id: string; force?: boolean }
  | {
      action: "close_worktree";
      project_id: string;
      merge?: boolean;
      stash?: boolean;
      fetch?: boolean;
      push?: boolean;
      delete_branch?: boolean;
    }
  | { action: "create_folder"; name: string }
  | { action: "delete_folder"; folder_id: string }
  | { action: "rename_folder"; folder_id: string; name: string }
  | { action: "move_project_to_folder"; project_id: string; folder_id: string; position?: number | null }
  | { action: "move_project_out_of_folder"; project_id: string; top_level_index: number }
  | { action: "move_project"; project_id: string; new_index: number }
  | { action: "move_item_in_order"; item_id: string; new_index: number }
  | { action: "toggle_project_pinned"; project_id: string }
  | { action: "reorder_worktree"; parent_id: string; worktree_id: string; new_index: number }
  | { action: "set_worktree_color_override"; project_id: string; color?: FolderColor | null }
  | { action: "load_session"; name: string }
  | { action: "save_session"; name: string }
  | { action: "import_workspace"; path: string }
  | { action: "export_workspace"; path: string }
  | { action: "get_settings" }
  | { action: "get_settings_schema" }
  | { action: "set_settings"; patch: JsonValue }
  | { action: "get_themes" }
  | { action: "get_theme"; id?: string | null }
  | { action: "set_theme"; id: string }
  | { action: "save_custom_theme"; id: string; config: JsonValue; activate?: boolean }
  | { action: "list_actions" }
  | { action: "invoke_action"; action_name: string; window?: string | null };

export interface PairRequest {
  code: string;
}

export interface PairResponse {
  token: string;
  expires_in: number;
}

export interface ErrorResponse {
  error: string;
}

// ── WebSocket types ─────────────────────────────────────────────────────────

// serde(tag = "type", rename_all = "snake_case")
export type WsInbound =
  | { type: "auth"; token: string }
  | { type: "subscribe"; terminal_ids: string[] }
  | { type: "unsubscribe"; terminal_ids: string[] }
  | { type: "send_text"; terminal_id: string; text: string }
  | { type: "send_special_key"; terminal_id: string; key: SpecialKey }
  | { type: "resize"; terminal_id: string; cols: number; rows: number }
  | { type: "ping" };

// serde(tag = "type", rename_all = "snake_case")
export type WsOutbound =
  | { type: "auth_ok" }
  | { type: "auth_failed"; error: string }
  | { type: "subscribed"; mappings: Record<string, number>; sizes?: Record<string, [number, number]> }
  | { type: "state_changed"; state_version: number }
  | { type: "dropped"; count: number }
  | { type: "pong" }
  | { type: "error"; error: string }
  | { type: "git_status_changed"; projects: Record<string, ApiGitStatus> }
  | { type: "toast"; id: string; level: string; message: string; detail?: string | null; ttl_ms: number; actions?: ApiToastAction[] }
  | { type: "terminal_resized"; terminal_id: string; cols: number; rows: number; server_owns?: boolean };

// Default PascalCase serialization
export type SpecialKey =
  | "Enter"
  | "Escape"
  | "Backspace"
  | "Delete"
  | "CtrlC"
  | "CtrlD"
  | "CtrlZ"
  | "Tab"
  | "ArrowUp"
  | "ArrowDown"
  | "ArrowLeft"
  | "ArrowRight"
  | "Home"
  | "End"
  | "PageUp"
  | "PageDown"
  | { Ctrl: string };

// ── Binary frame protocol ───────────────────────────────────────────────────

export const FRAME_TYPE_PTY = 1; // server → client: live PTY output
export const FRAME_TYPE_SNAPSHOT = 2; // server → client: full screen redraw
export const FRAME_TYPE_INPUT = 3; // client → server: terminal input

/** Parse a generic binary frame: [proto=1][frameType][streamId:u32BE][payload...] */
export function parseBinaryFrame(data: ArrayBuffer): { frameType: number; streamId: number; payload: Uint8Array } | null {
  const view = new DataView(data);
  if (data.byteLength < 6 || view.getUint8(0) !== 1) {
    return null;
  }
  const frameType = view.getUint8(1);
  const streamId = view.getUint32(2, false); // big-endian
  const payload = new Uint8Array(data, 6);
  return { frameType, streamId, payload };
}

/** Build a binary frame: [proto=1][frameType][streamId:u32BE][payload...] */
export function buildBinaryFrame(frameType: number, streamId: number, payload: Uint8Array): ArrayBuffer {
  const frame = new ArrayBuffer(6 + payload.length);
  const view = new DataView(frame);
  view.setUint8(0, 1); // proto version
  view.setUint8(1, frameType);
  view.setUint32(2, streamId, false); // big-endian
  new Uint8Array(frame, 6).set(payload);
  return frame;
}
