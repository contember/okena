/// Keyboard modifiers for terminal input conversion.
#[derive(Clone, Debug, Default)]
pub struct KeyModifiers {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
    /// Platform key (Cmd on macOS, Win on Windows/Linux)
    pub platform: bool,
}

/// A key event for terminal input conversion.
/// Framework-agnostic representation — convert from your UI framework's key events.
#[derive(Clone, Debug)]
pub struct KeyEvent {
    /// Key name (e.g. "a", "enter", "left", "f1")
    pub key: String,
    /// The character produced by the key, if any
    pub key_char: Option<String>,
    pub modifiers: KeyModifiers,
}

/// Active kitty keyboard protocol enhancement flags (read from the terminal mode).
/// Only `disambiguate_escape_codes` is honored today; the other progressive-
/// enhancement levels are a follow-up, so this struct intentionally carries
/// just that one flag for now.
#[derive(Clone, Copy, Debug, Default)]
pub struct KittyKeyboardFlags {
    pub disambiguate_escape_codes: bool,
}

/// Convert a key event to terminal input bytes.
///
/// `app_cursor_mode`: When true, arrow keys send SS3 sequences (\x1bOA) instead of CSI (\x1b[A).
/// This should be true when the terminal is in application cursor keys mode (DECCKM),
/// which is used by applications like less, vim, htop, etc.
///
/// `kitty`: Active kitty keyboard protocol flags. When `disambiguate_escape_codes`
/// is set, the ambiguous keys (Esc, ctrl/alt+key, modified Enter/Tab/Backspace) are
/// reported as `CSI u` sequences before the legacy logic runs.
pub fn key_to_bytes(
    event: &KeyEvent,
    app_cursor_mode: bool,
    kitty: KittyKeyboardFlags,
) -> Option<Vec<u8>> {
    if kitty.disambiguate_escape_codes
        && let Some(bytes) = kitty_disambiguate_bytes(event) {
        return Some(bytes);
    }

    let mods = &event.modifiers;

    // Handle Ctrl+key combinations for letters (produces control characters)
    if mods.control && !mods.shift && !mods.alt && !mods.platform {
        let key = event.key.as_str();
        if let Some(c) = key.chars().next()
            && key.len() == 1 && c.is_ascii_alphabetic() {
                let ctrl_char = (c.to_ascii_lowercase() as u8) - b'a' + 1;
                return Some(vec![ctrl_char]);
            }
    }

    // Handle Tab with modifiers
    if event.key.as_str() == "tab" {
        if mods.shift {
            // Shift+Tab (backtab)
            return Some(b"\x1b[Z".to_vec());
        }
        return Some(b"\t".to_vec());
    }

    // Handle Enter/Return with modifiers
    // Shift+Enter sends literal newline (for multi-line input in apps like Claude Code)
    // Regular Enter sends carriage return (submit)
    match event.key.as_str() {
        "enter" | "return" | "kp_enter" => {
            if mods.shift {
                return Some(b"\n".to_vec());
            }
            return Some(b"\r".to_vec());
        }
        _ => {}
    }

    // macOS-specific: Cmd+Arrow for line navigation
    // Cmd+Left = Ctrl+A (start of line), Cmd+Right = Ctrl+E (end of line)
    #[cfg(target_os = "macos")]
    if mods.platform && !mods.alt && !mods.control {
        match event.key.as_str() {
            "left" => return Some(vec![0x01]),
            "right" => return Some(vec![0x05]),
            "up" => return Some(b"\x1b[1;5A".to_vec()),
            "down" => return Some(b"\x1b[1;5B".to_vec()),
            "backspace" => return Some(vec![0x15]),
            _ => {}
        }
    }

    // macOS-specific: Option+Arrow for word navigation (readline sequences)
    // Option+Left = ESC b (word back), Option+Right = ESC f (word forward)
    #[cfg(target_os = "macos")]
    if mods.alt && !mods.platform && !mods.control {
        match event.key.as_str() {
            "left" => return Some(b"\x1bb".to_vec()),
            "right" => return Some(b"\x1bf".to_vec()),
            "backspace" => return Some(vec![0x17]),
            _ => {}
        }
    }

    // Calculate modifier code for CSI sequences
    // 1 = none, 2 = Shift, 3 = Alt, 4 = Shift+Alt, 5 = Ctrl, 6 = Shift+Ctrl, 7 = Alt+Ctrl, 8 = Shift+Alt+Ctrl
    let modifier_code = 1
        + (if mods.shift { 1 } else { 0 })
        + (if mods.alt { 2 } else { 0 })
        + (if mods.control { 4 } else { 0 });

    // Handle arrow keys with modifiers
    // In application cursor mode (DECCKM): use SS3 sequences (\x1bOA)
    // In normal mode: use CSI sequences (\x1b[A)
    // With modifiers: always use CSI 1;mod X format
    match event.key.as_str() {
        "up" | "down" | "right" | "left" => {
            let arrow_char = match event.key.as_str() {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                _ => unreachable!(),
            };
            if modifier_code > 1 {
                // Modifiers always use CSI format
                return Some(format!("\x1b[1;{}{}", modifier_code, arrow_char).into_bytes());
            }
            // No modifiers: use SS3 in app cursor mode, CSI otherwise
            if app_cursor_mode {
                return Some(format!("\x1bO{}", arrow_char).into_bytes());
            }
            return Some(format!("\x1b[{}", arrow_char).into_bytes());
        }
        _ => {}
    }

    // If the platform provides `key_char`, the UI framework will also deliver it via the
    // text-input (InputHandler) path. To avoid double-sending characters, let the InputHandler
    // handle all text-producing keystrokes.
    if event.key_char.is_some() {
        return None;
    }

    // Handle other special keys (with modifier support for some)
    match event.key.as_str() {
        "backspace" => return Some(b"\x7f".to_vec()),
        "escape" => return Some(b"\x1b".to_vec()),
        "home" => {
            if modifier_code > 1 {
                return Some(format!("\x1b[1;{}H", modifier_code).into_bytes());
            }
            return Some(b"\x1b[H".to_vec());
        }
        "end" => {
            if modifier_code > 1 {
                return Some(format!("\x1b[1;{}F", modifier_code).into_bytes());
            }
            return Some(b"\x1b[F".to_vec());
        }
        "pageup" => return Some(b"\x1b[5~".to_vec()),
        "pagedown" => return Some(b"\x1b[6~".to_vec()),
        "delete" => return Some(b"\x1b[3~".to_vec()),
        "f1" => return Some(b"\x1bOP".to_vec()),
        "f2" => return Some(b"\x1bOQ".to_vec()),
        "f3" => return Some(b"\x1bOR".to_vec()),
        "f4" => return Some(b"\x1bOS".to_vec()),
        "f5" => return Some(b"\x1b[15~".to_vec()),
        "f6" => return Some(b"\x1b[17~".to_vec()),
        "f7" => return Some(b"\x1b[18~".to_vec()),
        "f8" => return Some(b"\x1b[19~".to_vec()),
        "f9" => return Some(b"\x1b[20~".to_vec()),
        "f10" => return Some(b"\x1b[21~".to_vec()),
        "f11" => return Some(b"\x1b[23~".to_vec()),
        "f12" => return Some(b"\x1b[24~".to_vec()),
        _ => {}
    }

    // Single character keys as fallback
    let key = event.key.as_str();
    if key.len() == 1 {
        log::info!("Using key string: {:?}", key);
        return Some(key.as_bytes().to_vec());
    }

    log::warn!("No input generated for key: {:?}", event.key);
    None
}

/// Encode a key as a `CSI u` sequence: `CSI code u` when unmodified, or
/// `CSI code ; kmod u` when modifiers are held.
fn csi_u(code: u32, kmod: u32) -> Vec<u8> {
    if kmod == 1 {
        format!("\x1b[{code}u").into_bytes()
    } else {
        format!("\x1b[{code};{kmod}u").into_bytes()
    }
}

/// Kitty keyboard protocol level 1 ("Disambiguate escape codes").
///
/// Returns the `CSI u` encoding for the keys the disambiguate level rewrites:
/// Esc (always), modified Enter/Tab/Backspace, and ctrl/alt printable keys.
/// Returns `None` for everything else so the caller falls through to the
/// legacy logic, which already matches kitty for arrows/Home/End and produces
/// text for plain keys.
fn kitty_disambiguate_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    let mods = &event.modifiers;
    // Kitty modifier value: 1 + bitmask (shift=1, alt=2, ctrl=4, super=8).
    let kmod = 1
        + (if mods.shift { 1 } else { 0 })
        + (if mods.alt { 2 } else { 0 })
        + (if mods.control { 4 } else { 0 })
        + (if mods.platform { 8 } else { 0 });
    let has_mods = kmod > 1;

    match event.key.as_str() {
        // Plain Esc is reported as `CSI 27 u` so apps can tell it from the
        // start of an escape sequence.
        "escape" => Some(csi_u(27, kmod)),
        // Plain Enter stays legacy `\r`; modified Enter is disambiguated.
        "enter" | "return" | "kp_enter" => has_mods.then(|| csi_u(13, kmod)),
        // Plain Tab stays legacy `\t`; Shift+Tab → `\x1b[9;2u`.
        "tab" => has_mods.then(|| csi_u(9, kmod)),
        // Plain Backspace stays legacy; modified Backspace is disambiguated.
        "backspace" => has_mods.then(|| csi_u(127, kmod)),
        key => {
            // ctrl/alt + a single printable char (ctrl+letter, alt+key,
            // ctrl+alt+key, shift+alt+key). Plain super+key is a higher level.
            if (mods.control || mods.alt)
                && key.chars().count() == 1
                && let Some(c) = key.chars().next()
                && !c.is_control()
            {
                let cp = if c.is_ascii_alphabetic() {
                    c.to_ascii_lowercase() as u32
                } else {
                    c as u32
                };
                Some(csi_u(cp, kmod))
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(key: &str, key_char: Option<&str>, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            key: key.to_string(),
            key_char: key_char.map(|s| s.to_string()),
            modifiers: mods,
        }
    }

    fn ctrl() -> KeyModifiers {
        KeyModifiers { control: true, ..Default::default() }
    }

    fn shift() -> KeyModifiers {
        KeyModifiers { shift: true, ..Default::default() }
    }

    fn on() -> KittyKeyboardFlags {
        KittyKeyboardFlags { disambiguate_escape_codes: true }
    }

    #[test]
    fn flag_off_keeps_legacy_bytes() {
        let off = KittyKeyboardFlags::default();
        assert_eq!(
            key_to_bytes(&ev("a", None, ctrl()), false, off),
            Some(vec![0x01])
        );
        assert_eq!(
            key_to_bytes(&ev("tab", None, KeyModifiers::default()), false, off),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("escape", None, KeyModifiers::default()), false, off),
            Some(b"\x1b".to_vec())
        );
    }

    #[test]
    fn escape_is_disambiguated() {
        assert_eq!(
            key_to_bytes(&ev("escape", None, KeyModifiers::default()), false, on()),
            Some(b"\x1b[27u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("escape", None, ctrl()), false, on()),
            Some(b"\x1b[27;5u".to_vec())
        );
    }

    #[test]
    fn ctrl_letters_are_disambiguated() {
        assert_eq!(
            key_to_bytes(&ev("i", None, ctrl()), false, on()),
            Some(b"\x1b[105;5u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("a", None, ctrl()), false, on()),
            Some(b"\x1b[97;5u".to_vec())
        );
    }

    #[test]
    fn tab_disambiguation() {
        assert_eq!(
            key_to_bytes(&ev("tab", None, shift()), false, on()),
            Some(b"\x1b[9;2u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("tab", None, KeyModifiers::default()), false, on()),
            Some(b"\t".to_vec())
        );
    }

    #[test]
    fn enter_disambiguation() {
        assert_eq!(
            key_to_bytes(&ev("enter", None, ctrl()), false, on()),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("enter", None, KeyModifiers::default()), false, on()),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn ctrl_backspace_is_disambiguated() {
        assert_eq!(
            key_to_bytes(&ev("backspace", None, ctrl()), false, on()),
            Some(b"\x1b[127;5u".to_vec())
        );
    }

    #[test]
    fn plain_char_falls_through_to_text() {
        assert_eq!(
            key_to_bytes(&ev("a", Some("a"), KeyModifiers::default()), false, on()),
            None
        );
    }

    #[test]
    fn plain_arrow_is_not_stolen() {
        assert_eq!(
            key_to_bytes(&ev("up", None, KeyModifiers::default()), false, on()),
            Some(b"\x1b[A".to_vec())
        );
    }
}
