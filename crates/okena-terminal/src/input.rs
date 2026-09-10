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

/// Terminal modes and user settings that change how a keystroke is encoded.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyEncodeOptions {
    /// Application cursor keys mode (DECCKM): arrows send SS3 (`\x1bOA`) instead
    /// of CSI (`\x1b[A`). Set by applications like less, vim and htop.
    pub app_cursor_mode: bool,
    /// Active kitty keyboard protocol flags. With `disambiguate_escape_codes`,
    /// the ambiguous keys (Esc, ctrl/alt+key, modified Enter/Tab/Backspace) are
    /// reported as `CSI u` sequences before the legacy logic runs.
    pub kitty: KittyKeyboardFlags,
    /// macOS only: Option encodes Meta instead of composing a character. Callers
    /// must pair it with `consumes_composed_text`.
    pub option_as_meta: bool,
}

/// Convert a key event to terminal input bytes.
pub fn key_to_bytes(event: &KeyEvent, options: KeyEncodeOptions) -> Option<Vec<u8>> {
    if options.kitty.disambiguate_escape_codes
        && let Some(bytes) = kitty_disambiguate_bytes(event, options)
    {
        return Some(bytes);
    }

    let mods = &event.modifiers;

    // Handle Ctrl+key combinations (produces control characters).
    // Ctrl+Alt keeps the meta ESC prefix in front of the control character.
    if mods.control
        && !mods.platform
        && !delivered_as_text(event, options)
        && let Some(byte) = control_byte(&event.key)
    {
        if mods.alt {
            return Some(vec![0x1b, byte]);
        }
        return Some(vec![byte]);
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
            if options.app_cursor_mode {
                return Some(format!("\x1bO{}", arrow_char).into_bytes());
            }
            return Some(format!("\x1b[{}", arrow_char).into_bytes());
        }
        _ => {}
    }

    // The platform key drives app shortcuts, never PTY input, and text-producing
    // keystrokes are delivered again through the InputHandler path.
    if mods.platform || delivered_as_text(event, options) {
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
        _ => {}
    }

    if let Some(code) = tilde_key_code(&event.key) {
        if modifier_code > 1 {
            return Some(format!("\x1b[{};{}~", code, modifier_code).into_bytes());
        }
        return Some(format!("\x1b[{}~", code).into_bytes());
    }

    if let Some(final_byte) = ss3_function_key(&event.key) {
        if modifier_code > 1 {
            return Some(format!("\x1b[1;{}{}", modifier_code, final_byte).into_bytes());
        }
        return Some(format!("\x1bO{}", final_byte).into_bytes());
    }

    // Legacy meta: ESC prefix in front of the character the key sends on its own.
    if mods.alt
        && let Some(c) = meta_char(event)
    {
        return Some(format!("\x1b{}", c).into_bytes());
    }

    // Single character keys as fallback
    let key = event.key.as_str();
    if !mods.control && !mods.alt && key.len() == 1 {
        log::info!("Using key string: {:?}", key);
        return Some(key.as_bytes().to_vec());
    }

    log::warn!("No input generated for key: {:?}", event.key);
    None
}

/// True when the UI framework also commits this keystroke through the text-input
/// (InputHandler) path, where encoding it here as well would double-send it.
fn delivered_as_text(event: &KeyEvent, options: KeyEncodeOptions) -> bool {
    if !committable_text(event) {
        return false;
    }
    let mods = &event.modifiers;
    match (mods.control, mods.alt) {
        (false, false) => true,
        // Windows AltGr composes a character out of Ctrl+Alt and commits it via WM_CHAR.
        (true, true) => true,
        // macOS Option composes one (Option+B is `∫`) and commits it via the IME,
        // unless the user asked for Meta instead.
        (false, true) => cfg!(target_os = "macos") && !options.option_as_meta,
        (true, false) => false,
    }
}

/// True when `key_char` carries a character the platform can commit as text.
fn committable_text(event: &KeyEvent) -> bool {
    event
        .key_char
        .as_deref()
        .is_some_and(|text| !text.is_empty() && !text.chars().any(char::is_control))
}

/// True when the encoder claimed a keystroke macOS will *also* commit as text
/// (Option-as-Meta). Callers must stop propagation for exactly these, or the
/// composed character is sent on top of the meta sequence.
pub fn consumes_composed_text(event: &KeyEvent, options: KeyEncodeOptions) -> bool {
    let mods = &event.modifiers;
    cfg!(target_os = "macos")
        && options.option_as_meta
        && mods.alt
        && !mods.control
        && !mods.platform
        && committable_text(event)
}

/// The control character a Ctrl-modified key produces in the legacy encoding.
fn control_byte(key: &str) -> Option<u8> {
    if key == "space" {
        return Some(0x00);
    }
    let &[byte] = key.as_bytes() else {
        return None;
    };
    Some(match byte {
        b'a'..=b'z' | b'A'..=b'Z' => byte.to_ascii_lowercase() - b'a' + 1,
        b' ' | b'@' => 0x00,
        b'[' => 0x1b,
        b'\\' => 0x1c,
        b']' => 0x1d,
        b'^' => 0x1e,
        b'_' | b'/' => 0x1f,
        b'?' => 0x7f,
        _ => return None,
    })
}

/// Parameter of the xterm `CSI n ~` keys, which take `CSI n ; mod ~` when modified.
fn tilde_key_code(key: &str) -> Option<u32> {
    Some(match key {
        "insert" => 2,
        "delete" => 3,
        "pageup" => 5,
        "pagedown" => 6,
        "f5" => 15,
        "f6" => 17,
        "f7" => 18,
        "f8" => 19,
        "f9" => 20,
        "f10" => 21,
        "f11" => 23,
        "f12" => 24,
        _ => return None,
    })
}

/// Final byte of F1-F4, which are SS3 when unmodified and `CSI 1 ; mod X` otherwise.
fn ss3_function_key(key: &str) -> Option<char> {
    Some(match key {
        "f1" => 'P',
        "f2" => 'Q',
        "f3" => 'R',
        "f4" => 'S',
        _ => return None,
    })
}

/// The character an Alt-modified key sends after the meta ESC prefix. Reads `key`,
/// not `key_char`, so a layout's Alt composition never leaks into the sequence.
fn meta_char(event: &KeyEvent) -> Option<char> {
    if event.key == "space" {
        return Some(' ');
    }
    let mut chars = event.key.chars();
    let c = chars.next()?;
    if chars.next().is_some() || c.is_control() {
        return None;
    }
    Some(if event.modifiers.shift {
        c.to_ascii_uppercase()
    } else {
        c
    })
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
/// text for plain keys. A printable key the platform commits as text (AltGr,
/// macOS Option) is declined too, so the protocol never steals a composition.
fn kitty_disambiguate_bytes(event: &KeyEvent, options: KeyEncodeOptions) -> Option<Vec<u8>> {
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
                && !delivered_as_text(event, options)
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
        KeyModifiers {
            control: true,
            ..Default::default()
        }
    }

    fn shift() -> KeyModifiers {
        KeyModifiers {
            shift: true,
            ..Default::default()
        }
    }

    fn alt() -> KeyModifiers {
        KeyModifiers {
            alt: true,
            ..Default::default()
        }
    }

    fn on() -> KeyEncodeOptions {
        KeyEncodeOptions {
            kitty: KittyKeyboardFlags {
                disambiguate_escape_codes: true,
            },
            ..Default::default()
        }
    }

    fn off() -> KeyEncodeOptions {
        KeyEncodeOptions::default()
    }

    fn meta() -> KeyEncodeOptions {
        KeyEncodeOptions {
            option_as_meta: true,
            ..Default::default()
        }
    }

    fn legacy(key: &str, key_char: Option<&str>, mods: KeyModifiers) -> Option<Vec<u8>> {
        key_to_bytes(&ev(key, key_char, mods), off())
    }

    #[test]
    fn flag_off_keeps_legacy_bytes() {
        let off = KeyEncodeOptions::default();
        assert_eq!(key_to_bytes(&ev("a", None, ctrl()), off), Some(vec![0x01]));
        assert_eq!(
            key_to_bytes(&ev("tab", None, KeyModifiers::default()), off),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("escape", None, KeyModifiers::default()), off),
            Some(b"\x1b".to_vec())
        );
    }

    #[test]
    fn escape_is_disambiguated() {
        assert_eq!(
            key_to_bytes(&ev("escape", None, KeyModifiers::default()), on()),
            Some(b"\x1b[27u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("escape", None, ctrl()), on()),
            Some(b"\x1b[27;5u".to_vec())
        );
    }

    #[test]
    fn ctrl_letters_are_disambiguated() {
        assert_eq!(
            key_to_bytes(&ev("i", None, ctrl()), on()),
            Some(b"\x1b[105;5u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("a", None, ctrl()), on()),
            Some(b"\x1b[97;5u".to_vec())
        );
    }

    #[test]
    fn tab_disambiguation() {
        assert_eq!(
            key_to_bytes(&ev("tab", None, shift()), on()),
            Some(b"\x1b[9;2u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("tab", None, KeyModifiers::default()), on()),
            Some(b"\t".to_vec())
        );
    }

    #[test]
    fn enter_disambiguation() {
        assert_eq!(
            key_to_bytes(&ev("enter", None, ctrl()), on()),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            key_to_bytes(&ev("enter", None, KeyModifiers::default()), on()),
            Some(b"\r".to_vec())
        );
    }

    #[test]
    fn ctrl_backspace_is_disambiguated() {
        assert_eq!(
            key_to_bytes(&ev("backspace", None, ctrl()), on()),
            Some(b"\x1b[127;5u".to_vec())
        );
    }

    #[test]
    fn plain_char_falls_through_to_text() {
        assert_eq!(
            key_to_bytes(&ev("a", Some("a"), KeyModifiers::default()), on()),
            None
        );
    }

    #[test]
    fn plain_arrow_is_not_stolen() {
        assert_eq!(
            key_to_bytes(&ev("up", None, KeyModifiers::default()), on()),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn alt_printable_gets_the_meta_escape_prefix() {
        assert_eq!(legacy("b", None, alt()), Some(b"\x1bb".to_vec()));
        assert_eq!(legacy("f", None, alt()), Some(b"\x1bf".to_vec()));
        assert_eq!(legacy("space", None, alt()), Some(b"\x1b ".to_vec()));
        assert_eq!(
            legacy(
                "b",
                None,
                KeyModifiers {
                    alt: true,
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(b"\x1bB".to_vec())
        );
    }

    #[test]
    fn ctrl_alt_printable_prefixes_the_control_character() {
        assert_eq!(
            legacy(
                "b",
                None,
                KeyModifiers {
                    control: true,
                    alt: true,
                    ..Default::default()
                }
            ),
            Some(vec![0x1b, 0x02])
        );
    }

    /// Windows AltGr arrives as Ctrl+Alt and commits its character via WM_CHAR.
    #[test]
    fn altgr_composition_is_left_to_the_text_path() {
        assert_eq!(
            legacy(
                "q",
                Some("@"),
                KeyModifiers {
                    control: true,
                    alt: true,
                    ..Default::default()
                }
            ),
            None
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn alt_printable_is_encoded_even_when_the_platform_reports_a_char() {
        assert_eq!(legacy("b", Some("b"), alt()), Some(b"\x1bb".to_vec()));
    }

    /// macOS Option composes a character and commits it through the IME.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_composition_is_left_to_the_text_path() {
        assert_eq!(legacy("b", Some("\u{222b}"), alt()), None);
    }

    #[test]
    fn ctrl_space_is_nul() {
        assert_eq!(legacy("space", None, ctrl()), Some(vec![0x00]));
    }

    #[test]
    fn ctrl_punctuation_maps_to_control_characters() {
        assert_eq!(legacy("[", None, ctrl()), Some(vec![0x1b]));
        assert_eq!(legacy("\\", None, ctrl()), Some(vec![0x1c]));
        assert_eq!(legacy("]", None, ctrl()), Some(vec![0x1d]));
        assert_eq!(legacy("^", None, ctrl()), Some(vec![0x1e]));
        assert_eq!(legacy("_", None, ctrl()), Some(vec![0x1f]));
        assert_eq!(legacy("/", None, ctrl()), Some(vec![0x1f]));
        assert_eq!(legacy("?", None, ctrl()), Some(vec![0x7f]));
    }

    #[test]
    fn ctrl_shift_letter_still_maps_to_its_control_character() {
        assert_eq!(
            legacy(
                "a",
                None,
                KeyModifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(vec![0x01])
        );
    }

    #[test]
    fn tilde_keys_carry_their_modifier_parameter() {
        assert_eq!(legacy("pageup", None, shift()), Some(b"\x1b[5;2~".to_vec()));
        assert_eq!(legacy("delete", None, ctrl()), Some(b"\x1b[3;5~".to_vec()));
        assert_eq!(legacy("pagedown", None, alt()), Some(b"\x1b[6;3~".to_vec()));
        assert_eq!(
            legacy("pageup", None, KeyModifiers::default()),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            legacy("insert", None, KeyModifiers::default()),
            Some(b"\x1b[2~".to_vec())
        );
    }

    #[test]
    fn function_keys_keep_their_unmodified_encoding() {
        assert_eq!(
            legacy("f1", None, KeyModifiers::default()),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            legacy("f4", None, KeyModifiers::default()),
            Some(b"\x1bOS".to_vec())
        );
        assert_eq!(
            legacy("f5", None, KeyModifiers::default()),
            Some(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            legacy("f12", None, KeyModifiers::default()),
            Some(b"\x1b[24~".to_vec())
        );
    }

    #[test]
    fn modified_function_keys_carry_their_modifier_parameter() {
        assert_eq!(legacy("f1", None, ctrl()), Some(b"\x1b[1;5P".to_vec()));
        assert_eq!(legacy("f4", None, shift()), Some(b"\x1b[1;2S".to_vec()));
        assert_eq!(legacy("f5", None, shift()), Some(b"\x1b[15;2~".to_vec()));
        assert_eq!(legacy("f12", None, alt()), Some(b"\x1b[24;3~".to_vec()));
    }

    #[test]
    fn plain_char_is_never_double_sent() {
        assert_eq!(legacy("a", Some("a"), KeyModifiers::default()), None);
        assert_eq!(legacy("a", Some("A"), shift()), None);
        assert_eq!(legacy("space", Some(" "), KeyModifiers::default()), None);
    }

    #[test]
    fn platform_modified_keys_stay_out_of_the_pty() {
        let platform = KeyModifiers {
            platform: true,
            ..Default::default()
        };
        assert_eq!(legacy("b", Some("b"), platform.clone()), None);
        assert_eq!(legacy("f5", None, platform), None);
    }

    #[test]
    fn kitty_disambiguation_still_wins_over_the_legacy_meta_prefix() {
        assert_eq!(
            key_to_bytes(&ev("b", None, alt()), on()),
            Some(b"\x1b[98;3u".to_vec())
        );
    }

    fn ctrl_alt() -> KeyModifiers {
        KeyModifiers {
            control: true,
            alt: true,
            ..Default::default()
        }
    }

    fn encode(
        key: &str,
        key_char: Option<&str>,
        mods: KeyModifiers,
        options: KeyEncodeOptions,
    ) -> Option<Vec<u8>> {
        key_to_bytes(&ev(key, key_char, mods), options)
    }

    #[cfg(target_os = "macos")]
    fn kitty_meta() -> KeyEncodeOptions {
        KeyEncodeOptions {
            option_as_meta: true,
            ..on()
        }
    }

    /// Windows AltGr arrives as Ctrl+Alt; the kitty path must decline it too,
    /// or the negotiated protocol swallows every composed character.
    #[test]
    fn kitty_leaves_altgr_composition_to_the_text_path() {
        assert_eq!(encode("q", Some("@"), ctrl_alt(), on()), None);
    }

    #[test]
    fn kitty_still_disambiguates_ctrl_alt_that_composes_nothing() {
        assert_eq!(
            encode("q", None, ctrl_alt(), on()),
            Some(b"\x1b[113;7u".to_vec())
        );
        assert_eq!(
            encode("c", Some("\u{3}"), ctrl(), on()),
            Some(b"\x1b[99;5u".to_vec())
        );
    }

    /// Esc/Enter/Tab/Backspace only ever carry a control `key_char`, so the
    /// text gate must not reach them: their kitty encodings stay unconditional.
    #[test]
    fn kitty_control_keys_are_not_gated_by_the_text_path() {
        assert_eq!(
            encode("escape", Some("\u{1b}"), KeyModifiers::default(), on()),
            Some(b"\x1b[27u".to_vec())
        );
        assert_eq!(
            encode("enter", Some("\r"), ctrl(), on()),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            encode("tab", Some("\t"), shift(), on()),
            Some(b"\x1b[9;2u".to_vec())
        );
        assert_eq!(
            encode("backspace", Some("\u{7f}"), ctrl(), on()),
            Some(b"\x1b[127;5u".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_as_meta_encodes_the_composed_key() {
        assert_eq!(
            encode("b", Some("\u{222b}"), alt(), meta()),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            encode("space", Some("\u{a0}"), alt(), meta()),
            Some(b"\x1b ".to_vec())
        );
        assert_eq!(
            encode("b", Some("\u{222b}"), alt(), kitty_meta()),
            Some(b"\x1b[98;3u".to_vec())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_option_as_meta_marks_the_keystroke_as_consumed() {
        assert!(consumes_composed_text(
            &ev("b", Some("\u{222b}"), alt()),
            meta()
        ));
        assert!(consumes_composed_text(
            &ev("b", Some("\u{222b}"), alt()),
            kitty_meta()
        ));
        assert!(!consumes_composed_text(
            &ev("b", Some("\u{222b}"), alt()),
            off()
        ));
    }

    /// Option-as-Meta is a macOS-only escape hatch; elsewhere Alt is already
    /// encoded and nothing else commits the keystroke.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn option_as_meta_changes_nothing_off_macos() {
        assert_eq!(
            encode("b", Some("b"), alt(), off()),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            encode("b", Some("b"), alt(), meta()),
            Some(b"\x1bb".to_vec())
        );
        assert!(!consumes_composed_text(&ev("b", Some("b"), alt()), meta()));
    }

    #[test]
    fn nothing_else_is_reported_as_consumed_text() {
        assert!(!consumes_composed_text(
            &ev("a", Some("a"), KeyModifiers::default()),
            meta()
        ));
        assert!(!consumes_composed_text(&ev("left", None, alt()), meta()));
        assert!(!consumes_composed_text(
            &ev("q", Some("@"), ctrl_alt()),
            meta()
        ));
    }
}
