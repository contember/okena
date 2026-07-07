pub mod config;
pub mod connection;
pub mod id;
pub mod state;
pub mod terminal;
pub mod tls;
pub mod types;

pub use config::{LocalEndpoint, RemoteConnectionConfig, LOCAL_DAEMON_CONNECTION_ID};
pub use connection::{ConnectionHandler, RemoteClient};
pub use id::{is_remote_terminal, make_prefixed_id, strip_prefix};
pub use state::{
    collect_all_terminal_ids, collect_layout_terminal_ids, collect_state_terminal_ids,
    collect_terminal_sizes, diff_states, StateDiff,
};
pub use terminal::{
    close_remote_terminal, resize_remote_terminal, send_remote_terminal_input,
    REMOTE_TERMINAL_ANSWERS_QUERIES, REMOTE_TERMINAL_RESIZE_DEBOUNCE_MS,
    REMOTE_TERMINAL_USES_MOUSE_BACKEND,
};
pub use types::{ConnectionEvent, ConnectionStatus, WsClientMessage, TOKEN_REFRESH_AGE_SECS};
