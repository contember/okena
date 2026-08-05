pub mod auth;
pub mod bridge;
pub mod local;
pub mod pty_broadcaster;
pub mod routes;
pub mod serve;
pub mod server;
pub mod tls;
pub mod types;

// The remote server runs in the daemon process, so nothing in-process describes
// its status. Clients read the daemon's `remote.json` (see [`local`]) and its
// REST API instead.
