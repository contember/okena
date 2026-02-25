use crate::client::terminal_holder::TerminalHolder;

use okena_core::client::{is_remote_terminal, ConnectionHandler, WsClientMessage};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// Mobile-specific handler that creates `TerminalHolder` objects.
///
/// The `terminals` map is shared with the FFI layer so `get_visible_cells` / `get_cursor`
/// can read from it.
pub struct MobileConnectionHandler {
    terminals: Arc<RwLock<HashMap<String, TerminalHolder>>>,
    last_activity: Mutex<Instant>,
}

impl MobileConnectionHandler {
    pub fn new(terminals: Arc<RwLock<HashMap<String, TerminalHolder>>>) -> Self {
        Self {
            terminals,
            last_activity: Mutex::new(Instant::now()),
        }
    }

    pub fn terminals(&self) -> &Arc<RwLock<HashMap<String, TerminalHolder>>> {
        &self.terminals
    }

    /// Seconds since last WS activity (terminal output).
    pub fn seconds_since_activity(&self) -> f64 {
        self.last_activity.lock().elapsed().as_secs_f64()
    }
}

impl ConnectionHandler for MobileConnectionHandler {
    fn create_terminal(
        &self,
        _connection_id: &str,
        _terminal_id: &str,
        prefixed_id: &str,
        _ws_sender: async_channel::Sender<WsClientMessage>,
    ) {
        // Skip if terminal already exists — avoids leaking the old TerminalHolder
        // (and its alacritty grid) on reconnect when the server re-sends creates.
        if self.terminals.read().contains_key(prefixed_id) {
            return;
        }
        let holder = TerminalHolder::new(80, 24);
        self.terminals
            .write()
            .insert(prefixed_id.to_string(), holder);
    }

    fn on_terminal_output(&self, prefixed_id: &str, data: &[u8]) {
        *self.last_activity.lock() = Instant::now();
        if let Some(holder) = self.terminals.read().get(prefixed_id) {
            holder.process_output(data);
        }
    }

    fn resize_terminal(&self, prefixed_id: &str, cols: u16, rows: u16) {
        if let Some(holder) = self.terminals.read().get(prefixed_id) {
            holder.resize(cols, rows);
        }
    }

    fn remove_terminal(&self, prefixed_id: &str) {
        self.terminals.write().remove(prefixed_id);
    }

    fn remove_all_terminals(&self, connection_id: &str) {
        let mut terminals = self.terminals.write();
        let to_remove: Vec<String> = terminals
            .keys()
            .filter(|k| is_remote_terminal(k, connection_id))
            .cloned()
            .collect();
        for key in to_remove {
            terminals.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handler() -> MobileConnectionHandler {
        MobileConnectionHandler::new(Arc::new(RwLock::new(HashMap::new())))
    }

    #[test]
    fn create_and_remove_terminal() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx);
        assert!(handler.terminals().read().contains_key("remote:conn1:t1"));

        handler.remove_terminal("remote:conn1:t1");
        assert!(!handler.terminals().read().contains_key("remote:conn1:t1"));
    }

    #[test]
    fn create_terminal_is_idempotent() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx.clone());
        let ptr1 = {
            let terminals = handler.terminals().read();
            terminals.get("remote:conn1:t1").unwrap() as *const TerminalHolder
        };

        // Second create with same prefixed_id should be a no-op
        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx);
        let ptr2 = {
            let terminals = handler.terminals().read();
            terminals.get("remote:conn1:t1").unwrap() as *const TerminalHolder
        };

        assert_eq!(ptr1, ptr2, "second create should reuse existing terminal");
        assert_eq!(handler.terminals().read().len(), 1);
    }

    #[test]
    fn remove_all_for_connection() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx.clone());
        handler.create_terminal("conn1", "t2", "remote:conn1:t2", tx.clone());
        handler.create_terminal("conn2", "t3", "remote:conn2:t3", tx);

        handler.remove_all_terminals("conn1");

        let terminals = handler.terminals().read();
        assert!(!terminals.contains_key("remote:conn1:t1"));
        assert!(!terminals.contains_key("remote:conn1:t2"));
        assert!(terminals.contains_key("remote:conn2:t3"));
    }

    #[test]
    fn on_terminal_output_routes_data() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx);
        handler.on_terminal_output("remote:conn1:t1", b"hello");

        let terminals = handler.terminals().read();
        let holder = terminals.get("remote:conn1:t1").unwrap();
        assert!(holder.is_dirty());
    }
}
