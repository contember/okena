use crate::client::id::strip_prefix;
use crate::client::types::WsClientMessage;

pub const REMOTE_TERMINAL_USES_MOUSE_BACKEND: bool = false;
pub const REMOTE_TERMINAL_RESIZE_DEBOUNCE_MS: u64 = 150;
pub const REMOTE_TERMINAL_ANSWERS_QUERIES: bool = false;

pub fn send_remote_terminal_input(
    ws_tx: &async_channel::Sender<WsClientMessage>,
    connection_id: &str,
    terminal_id: &str,
    data: &[u8],
) {
    let remote_id = strip_prefix(terminal_id, connection_id);
    let _ = ws_tx.try_send(WsClientMessage::SendText {
        terminal_id: remote_id,
        text: String::from_utf8_lossy(data).to_string(),
    });
}

pub fn resize_remote_terminal(
    ws_tx: &async_channel::Sender<WsClientMessage>,
    connection_id: &str,
    terminal_id: &str,
    cols: u16,
    rows: u16,
) {
    let remote_id = strip_prefix(terminal_id, connection_id);
    let _ = ws_tx.try_send(WsClientMessage::Resize {
        terminal_id: remote_id,
        cols,
        rows,
    });
}

pub fn close_remote_terminal(
    ws_tx: &async_channel::Sender<WsClientMessage>,
    connection_id: &str,
    terminal_id: &str,
) {
    let remote_id = strip_prefix(terminal_id, connection_id);
    let _ = ws_tx.try_send(WsClientMessage::CloseTerminal {
        terminal_id: remote_id,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_remote_terminal_input_strips_prefix_and_encodes_lossy_utf8() {
        let (tx, rx) = async_channel::bounded(1);

        send_remote_terminal_input(&tx, "conn-1", "remote:conn-1:term-a", b"a\xff");

        match rx.try_recv().expect("message queued") {
            WsClientMessage::SendText { terminal_id, text } => {
                assert_eq!(terminal_id, "term-a");
                assert_eq!(text, "a\u{fffd}");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn resize_remote_terminal_strips_prefix() {
        let (tx, rx) = async_channel::bounded(1);

        resize_remote_terminal(&tx, "conn-1", "remote:conn-1:term-a", 120, 40);

        match rx.try_recv().expect("message queued") {
            WsClientMessage::Resize {
                terminal_id,
                cols,
                rows,
            } => {
                assert_eq!(terminal_id, "term-a");
                assert_eq!(cols, 120);
                assert_eq!(rows, 40);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn close_remote_terminal_strips_prefix() {
        let (tx, rx) = async_channel::bounded(1);

        close_remote_terminal(&tx, "conn-1", "remote:conn-1:term-a");

        match rx.try_recv().expect("message queued") {
            WsClientMessage::CloseTerminal { terminal_id } => {
                assert_eq!(terminal_id, "term-a");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
