use crate::client::terminal_holder::TerminalHolder;

use okena_core::client::{is_remote_terminal, ConnectionHandler, WsClientMessage};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Mobile-specific handler that creates `TerminalHolder` objects.
///
/// The `terminals` map is shared with the FFI layer so `get_visible_cells` / `get_cursor`
/// can read from it.
pub struct MobileConnectionHandler {
    terminals: Arc<RwLock<HashMap<String, TerminalHolder>>>,
}

impl MobileConnectionHandler {
    pub fn new(terminals: Arc<RwLock<HashMap<String, TerminalHolder>>>) -> Self {
        Self { terminals }
    }

    pub fn terminals(&self) -> &Arc<RwLock<HashMap<String, TerminalHolder>>> {
        &self.terminals
    }
}

impl ConnectionHandler for MobileConnectionHandler {
    fn create_terminal(
        &self,
        _connection_id: &str,
        _terminal_id: &str,
        prefixed_id: &str,
        _ws_sender: async_channel::Sender<WsClientMessage>,
        cols: u16,
        rows: u16,
    ) {
        let (c, r) = if cols > 0 && rows > 0 {
            (cols, rows)
        } else {
            (80, 24)
        };
        let holder = TerminalHolder::new(c, r);
        self.terminals
            .write()
            .insert(prefixed_id.to_string(), holder);
    }

    fn on_terminal_output(&self, prefixed_id: &str, data: &[u8]) {
        if let Some(holder) = self.terminals.read().get(prefixed_id) {
            holder.process_output(data);
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

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx, 80, 24);
        assert!(handler.terminals().read().contains_key("remote:conn1:t1"));

        handler.remove_terminal("remote:conn1:t1");
        assert!(!handler.terminals().read().contains_key("remote:conn1:t1"));
    }

    #[test]
    fn remove_all_for_connection() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx.clone(), 80, 24);
        handler.create_terminal("conn1", "t2", "remote:conn1:t2", tx.clone(), 80, 24);
        handler.create_terminal("conn2", "t3", "remote:conn2:t3", tx, 80, 24);

        handler.remove_all_terminals("conn1");

        let terminals = handler.terminals().read();
        assert!(!terminals.contains_key("remote:conn1:t1"));
        assert!(!terminals.contains_key("remote:conn1:t2"));
        assert!(terminals.contains_key("remote:conn2:t3"));
    }

    #[test]
    fn create_terminal_uses_server_size() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx, 160, 48);
        let terminals = handler.terminals().read();
        let holder = terminals.get("remote:conn1:t1").unwrap();
        let cells = holder.get_visible_cells(&okena_core::theme::DARK_THEME);
        assert_eq!(cells.len(), 160 * 48);
    }

    #[test]
    fn create_terminal_falls_back_to_default_on_zero_size() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx, 0, 0);
        let terminals = handler.terminals().read();
        let holder = terminals.get("remote:conn1:t1").unwrap();
        let cells = holder.get_visible_cells(&okena_core::theme::DARK_THEME);
        assert_eq!(cells.len(), 80 * 24);
    }

    #[test]
    fn on_terminal_output_routes_data() {
        let handler = make_handler();
        let (tx, _rx) = async_channel::bounded(1);

        handler.create_terminal("conn1", "t1", "remote:conn1:t1", tx, 80, 24);
        handler.on_terminal_output("remote:conn1:t1", b"hello");

        let terminals = handler.terminals().read();
        let holder = terminals.get("remote:conn1:t1").unwrap();
        assert!(holder.is_dirty());
    }
}
