use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Named special keys the remote API supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SpecialKey {
    Enter,
    Escape,
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
    Backspace,
    Delete,
    /// A generic Ctrl-<char> chord (e.g. `Ctrl('l')` to clear). Serialized as
    /// `{"Ctrl":"l"}`. `CtrlC`/`CtrlD`/`CtrlZ` are kept as named variants for
    /// backward compatibility but are equivalent to `Ctrl('c')` etc.
    Ctrl(char),
}

impl SpecialKey {
    /// Convert to the byte sequence sent to the PTY.
    pub fn to_bytes(&self) -> Cow<'static, [u8]> {
        match self {
            SpecialKey::Enter => Cow::Borrowed(b"\r"),
            SpecialKey::Escape => Cow::Borrowed(b"\x1b"),
            SpecialKey::CtrlC => Cow::Borrowed(b"\x03"),
            SpecialKey::CtrlD => Cow::Borrowed(b"\x04"),
            SpecialKey::CtrlZ => Cow::Borrowed(b"\x1a"),
            SpecialKey::Tab => Cow::Borrowed(b"\t"),
            SpecialKey::ArrowUp => Cow::Borrowed(b"\x1b[A"),
            SpecialKey::ArrowDown => Cow::Borrowed(b"\x1b[B"),
            SpecialKey::ArrowRight => Cow::Borrowed(b"\x1b[C"),
            SpecialKey::ArrowLeft => Cow::Borrowed(b"\x1b[D"),
            SpecialKey::Home => Cow::Borrowed(b"\x1b[H"),
            SpecialKey::End => Cow::Borrowed(b"\x1b[F"),
            SpecialKey::PageUp => Cow::Borrowed(b"\x1b[5~"),
            SpecialKey::PageDown => Cow::Borrowed(b"\x1b[6~"),
            SpecialKey::Backspace => Cow::Borrowed(b"\x7f"),
            SpecialKey::Delete => Cow::Borrowed(b"\x1b[3~"),
            // ASCII control code: `c & 0x1f` (ctrl-a → 0x01 … ctrl-z → 0x1a,
            // and ctrl-[ → ESC, etc.). Non-ASCII chars have no control code.
            SpecialKey::Ctrl(c) if c.is_ascii() => Cow::Owned(vec![(*c as u8) & 0x1f]),
            SpecialKey::Ctrl(_) => Cow::Owned(Vec::new()),
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
            SpecialKey::Backspace,
            SpecialKey::Delete,
        ];
        for key in keys {
            let json = serde_json::to_string(&key).unwrap();
            let parsed: SpecialKey = serde_json::from_str(&json).unwrap();
            assert_eq!(key.to_bytes(), parsed.to_bytes());
        }
    }

    #[test]
    fn special_key_to_bytes() {
        assert_eq!(SpecialKey::Enter.to_bytes().as_ref(), b"\r");
        assert_eq!(SpecialKey::Escape.to_bytes().as_ref(), b"\x1b");
        assert_eq!(SpecialKey::CtrlC.to_bytes().as_ref(), b"\x03");
        assert_eq!(SpecialKey::CtrlD.to_bytes().as_ref(), b"\x04");
        assert_eq!(SpecialKey::CtrlZ.to_bytes().as_ref(), b"\x1a");
        assert_eq!(SpecialKey::Tab.to_bytes().as_ref(), b"\t");
        assert_eq!(SpecialKey::ArrowUp.to_bytes().as_ref(), b"\x1b[A");
        assert_eq!(SpecialKey::ArrowDown.to_bytes().as_ref(), b"\x1b[B");
        assert_eq!(SpecialKey::ArrowRight.to_bytes().as_ref(), b"\x1b[C");
        assert_eq!(SpecialKey::ArrowLeft.to_bytes().as_ref(), b"\x1b[D");
        assert_eq!(SpecialKey::Home.to_bytes().as_ref(), b"\x1b[H");
        assert_eq!(SpecialKey::End.to_bytes().as_ref(), b"\x1b[F");
        assert_eq!(SpecialKey::PageUp.to_bytes().as_ref(), b"\x1b[5~");
        assert_eq!(SpecialKey::PageDown.to_bytes().as_ref(), b"\x1b[6~");
        assert_eq!(SpecialKey::Backspace.to_bytes().as_ref(), b"\x7f");
        assert_eq!(SpecialKey::Delete.to_bytes().as_ref(), b"\x1b[3~");
    }

    #[test]
    fn ctrl_char_to_bytes_and_round_trip() {
        // ctrl-a..ctrl-z → 0x01..0x1a
        assert_eq!(SpecialKey::Ctrl('a').to_bytes().as_ref(), b"\x01");
        assert_eq!(SpecialKey::Ctrl('l').to_bytes().as_ref(), b"\x0c");
        assert_eq!(SpecialKey::Ctrl('z').to_bytes().as_ref(), b"\x1a");
        // Uppercase resolves to the same control code as lowercase.
        assert_eq!(
            SpecialKey::Ctrl('L').to_bytes(),
            SpecialKey::Ctrl('l').to_bytes()
        );
        // Equivalent to the named CtrlC variant.
        assert_eq!(SpecialKey::Ctrl('c').to_bytes(), SpecialKey::CtrlC.to_bytes());

        // Wire form: {"Ctrl":"l"} round-trips.
        let json = serde_json::to_string(&SpecialKey::Ctrl('l')).unwrap();
        assert_eq!(json, r#"{"Ctrl":"l"}"#);
        let parsed: SpecialKey = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.to_bytes().as_ref(), b"\x0c");
    }
}
