pub mod config;
pub mod connection;
pub mod id;
pub mod state;
pub mod terminal;
pub mod tls;
pub mod types;

pub use crate::{LOCAL_DAEMON_CONNECTION_ID, LocalEndpoint, RemoteConnectionConfig};
pub use connection::{ConnectionHandler, RemoteClient};
pub use id::{is_remote_terminal, make_prefixed_id, strip_prefix};
pub use state::{
    StateDiff, collect_all_terminal_ids, collect_layout_terminal_ids, collect_state_terminal_ids,
    collect_terminal_sizes, diff_states,
};
pub use terminal::{
    REMOTE_TERMINAL_ANSWERS_QUERIES, REMOTE_TERMINAL_RESIZE_DEBOUNCE_MS,
    REMOTE_TERMINAL_USES_MOUSE_BACKEND, close_remote_terminal, resize_remote_terminal,
    send_remote_terminal_input,
};
pub use types::{ConnectionEvent, ConnectionStatus, TOKEN_REFRESH_AGE_SECS, WsClientMessage};
