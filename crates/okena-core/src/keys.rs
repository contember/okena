use serde::{Deserialize, Serialize};

/// Named special keys the remote API supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpecialKey {
    Enter,
    Escape,
    Backspace,
    Delete,
    CtrlC,
    CtrlD,
    CtrlZ,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
}

impl SpecialKey {
    /// Convert to the byte sequence sent to the PTY.
    pub fn to_bytes(&self) -> &[u8] {
        match self {
            SpecialKey::Enter => b"\r",
            SpecialKey::Escape => b"\x1b",
            SpecialKey::Backspace => b"\x7f",
            SpecialKey::Delete => b"\x1b[3~",
            SpecialKey::CtrlC => b"\x03",
            SpecialKey::CtrlD => b"\x04",
            SpecialKey::CtrlZ => b"\x1a",
            SpecialKey::Tab => b"\t",
            SpecialKey::ArrowUp => b"\x1b[A",
            SpecialKey::ArrowDown => b"\x1b[B",
            SpecialKey::ArrowRight => b"\x1b[C",
            SpecialKey::ArrowLeft => b"\x1b[D",
            SpecialKey::Home => b"\x1b[H",
            SpecialKey::End => b"\x1b[F",
            SpecialKey::PageUp => b"\x1b[5~",
            SpecialKey::PageDown => b"\x1b[6~",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_key_round_trip() {
        let keys = vec![
            SpecialKey::Enter,
            SpecialKey::Escape,
            SpecialKey::Backspace,
            SpecialKey::Delete,
            SpecialKey::CtrlC,
            SpecialKey::CtrlD,
            SpecialKey::CtrlZ,
            SpecialKey::Tab,
            SpecialKey::ArrowUp,
            SpecialKey::ArrowDown,
            SpecialKey::ArrowLeft,
            SpecialKey::ArrowRight,
            SpecialKey::Home,
            SpecialKey::End,
            SpecialKey::PageUp,
            SpecialKey::PageDown,
        ];
        for key in keys {
            let json = serde_json::to_string(&key).unwrap();
            let parsed: SpecialKey = serde_json::from_str(&json).unwrap();
            assert_eq!(key.to_bytes(), parsed.to_bytes());
        }
    }

    #[test]
    fn special_key_to_bytes() {
        assert_eq!(SpecialKey::Enter.to_bytes(), b"\r");
        assert_eq!(SpecialKey::Escape.to_bytes(), b"\x1b");
        assert_eq!(SpecialKey::Backspace.to_bytes(), b"\x7f");
        assert_eq!(SpecialKey::Delete.to_bytes(), b"\x1b[3~");
        assert_eq!(SpecialKey::CtrlC.to_bytes(), b"\x03");
        assert_eq!(SpecialKey::CtrlD.to_bytes(), b"\x04");
        assert_eq!(SpecialKey::CtrlZ.to_bytes(), b"\x1a");
        assert_eq!(SpecialKey::Tab.to_bytes(), b"\t");
        assert_eq!(SpecialKey::ArrowUp.to_bytes(), b"\x1b[A");
        assert_eq!(SpecialKey::ArrowDown.to_bytes(), b"\x1b[B");
        assert_eq!(SpecialKey::ArrowRight.to_bytes(), b"\x1b[C");
        assert_eq!(SpecialKey::ArrowLeft.to_bytes(), b"\x1b[D");
        assert_eq!(SpecialKey::Home.to_bytes(), b"\x1b[H");
        assert_eq!(SpecialKey::End.to_bytes(), b"\x1b[F");
        assert_eq!(SpecialKey::PageUp.to_bytes(), b"\x1b[5~");
        assert_eq!(SpecialKey::PageDown.to_bytes(), b"\x1b[6~");
    }
}
