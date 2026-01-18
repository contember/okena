use gpui::KeyDownEvent;

/// Convert a GPUI key event to terminal input bytes
pub fn key_to_bytes(event: &KeyDownEvent) -> Option<Vec<u8>> {
    let keystroke = &event.keystroke;
    let mods = &keystroke.modifiers;

    // Debug: log all key events to help diagnose input issues
    log::debug!(
        "key_to_bytes: key={:?}, key_char={:?}, mods=(ctrl={}, shift={}, alt={}, platform={})",
        keystroke.key,
        keystroke.key_char,
        mods.control,
        mods.shift,
        mods.alt,
        mods.platform
    );

    // Handle Ctrl+key combinations for letters (produces control characters)
    if mods.control && !mods.shift && !mods.alt && !mods.platform {
        let key = keystroke.key.as_str();
        if key.len() == 1 {
            let c = key.chars().next().unwrap();
            if c.is_ascii_alphabetic() {
                let ctrl_char = (c.to_ascii_lowercase() as u8) - b'a' + 1;
                return Some(vec![ctrl_char]);
            }
        }
    }

    // Handle Tab with modifiers
    if keystroke.key.as_str() == "tab" {
        if mods.shift {
            // Shift+Tab (backtab)
            return Some(b"\x1b[Z".to_vec());
        }
        return Some(b"\t".to_vec());
    }

    // Handle Enter/Return with modifiers
    // Shift+Enter sends literal newline (for multi-line input in apps like Claude Code)
    // Regular Enter sends carriage return (submit)
    match keystroke.key.as_str() {
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
        match keystroke.key.as_str() {
            "left" => {
                log::debug!("macOS Cmd+Left -> Ctrl+A (0x01)");
                return Some(vec![0x01]);
            }
            "right" => {
                log::debug!("macOS Cmd+Right -> Ctrl+E (0x05)");
                return Some(vec![0x05]);
            }
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
        match keystroke.key.as_str() {
            "left" => {
                log::debug!("macOS Option+Left -> ESC b");
                return Some(b"\x1bb".to_vec());
            }
            "right" => {
                log::debug!("macOS Option+Right -> ESC f");
                return Some(b"\x1bf".to_vec());
            }
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

    // Handle arrow keys with modifiers (CSI 1;mod X format)
    // On non-macOS, or when macOS handlers above didn't match
    match keystroke.key.as_str() {
        "up" | "down" | "right" | "left" => {
            let arrow_char = match keystroke.key.as_str() {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                _ => unreachable!(),
            };
            if modifier_code > 1 {
                return Some(format!("\x1b[1;{}{}", modifier_code, arrow_char).into_bytes());
            }
            return Some(format!("\x1b[{}", arrow_char).into_bytes());
        }
        _ => {}
    }

    // If the platform provides `key_char`, GPUI will also deliver it via the text-input
    // (InputHandler) path. To avoid double-sending characters, let the InputHandler
    // handle all text-producing keystrokes.
    if keystroke.key_char.is_some() {
        return None;
    }

    // Handle other special keys (with modifier support for some)
    match keystroke.key.as_str() {
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
    let key = keystroke.key.as_str();
    if key.len() == 1 {
        log::info!("Using key string: {:?}", key);
        return Some(key.as_bytes().to_vec());
    }

    log::warn!("No input generated for key: {:?}", keystroke.key);
    None
}
