//! Key code translation.
//!
//! Keys travel on the wire as USB HID usages (Keyboard/Keypad page 0x07), so each side
//! only needs one table: Windows scan code -> HID on the sender, HID -> macOS virtual
//! keycode on the receiver. Physical keys are sent, not characters, so the Mac's own
//! input source (US, JIS, ...) decides what a key types.

pub mod hid {
    //! The HID usages referred to by name elsewhere.
    pub const M: u16 = 0x10;
    pub const GRAVE: u16 = 0x35; // ` on US; 半角/全角 on JIS
    pub const CAPS_LOCK: u16 = 0x39;
    pub const INTL_RO: u16 = 0x87; // JIS ろ / _
    pub const INTL_KANA: u16 = 0x88; // JIS カタカナ/ひらがな
    pub const INTL_YEN: u16 = 0x89; // JIS ¥
    pub const INTL_HENKAN: u16 = 0x8A; // JIS 変換
    pub const INTL_MUHENKAN: u16 = 0x8B; // JIS 無変換
    pub const LANG1: u16 = 0x90;
    pub const LANG2: u16 = 0x91;
    pub const LEFT_CTRL: u16 = 0xE0;
    pub const LEFT_SHIFT: u16 = 0xE1;
    pub const LEFT_ALT: u16 = 0xE2;
    pub const LEFT_GUI: u16 = 0xE3; // Windows key / Cmd
    pub const RIGHT_CTRL: u16 = 0xE4;
    pub const RIGHT_SHIFT: u16 = 0xE5;
    pub const RIGHT_ALT: u16 = 0xE6;
    pub const RIGHT_GUI: u16 = 0xE7;

    pub fn is_ctrl(h: u16) -> bool {
        h == LEFT_CTRL || h == RIGHT_CTRL
    }
    pub fn is_alt(h: u16) -> bool {
        h == LEFT_ALT || h == RIGHT_ALT
    }
}

// Windows virtual-key codes needed where the scan code is ambiguous or missing.
const VK_PAUSE: u32 = 0x13;
const VK_NUMLOCK: u32 = 0x90;

/// Translate a low-level-hook keyboard event (scan code, LLKHF_EXTENDED flag, vk code)
/// into a HID usage. Returns `None` for keys we don't forward.
pub fn win_to_hid(scan: u32, extended: bool, vk: u32) -> Option<u16> {
    // Pause and NumLock share scan code 0x45 and their "extended" flag is reported
    // the opposite way round from every other key, so decide by virtual key.
    match vk {
        VK_PAUSE => return Some(0x48),
        VK_NUMLOCK => return Some(0x53),
        _ => {}
    }
    let h = if extended {
        match scan {
            0x1C => 0x58, // keypad Enter
            0x1D => 0xE4, // right Ctrl
            0x35 => 0x54, // keypad /
            0x36 => 0xE5, // right Shift (some drivers flag it extended)
            0x37 => 0x46, // Print Screen
            0x38 => 0xE6, // right Alt / AltGr
            0x47 => 0x4A, // Home
            0x48 => 0x52, // Up
            0x49 => 0x4B, // Page Up
            0x4B => 0x50, // Left
            0x4D => 0x4F, // Right
            0x4F => 0x4D, // End
            0x50 => 0x51, // Down
            0x51 => 0x4E, // Page Down
            0x52 => 0x49, // Insert
            0x53 => 0x4C, // Delete
            0x5B => 0xE3, // left Windows
            0x5C => 0xE7, // right Windows
            0x5D => 0x65, // Menu / Application
            _ => return None,
        }
    } else {
        match scan {
            0x01 => 0x29, // Esc
            0x02..=0x0A => 0x1E + (scan as u16 - 0x02), // 1..9
            0x0B => 0x27, // 0
            0x0C => 0x2D, // -
            0x0D => 0x2E, // =
            0x0E => 0x2A, // Backspace
            0x0F => 0x2B, // Tab
            0x10 => 0x14, // Q
            0x11 => 0x1A, // W
            0x12 => 0x08, // E
            0x13 => 0x15, // R
            0x14 => 0x17, // T
            0x15 => 0x1C, // Y
            0x16 => 0x18, // U
            0x17 => 0x0C, // I
            0x18 => 0x12, // O
            0x19 => 0x13, // P
            0x1A => 0x2F, // [
            0x1B => 0x30, // ]
            0x1C => 0x28, // Enter
            0x1D => 0xE0, // left Ctrl
            0x1E => 0x04, // A
            0x1F => 0x16, // S
            0x20 => 0x07, // D
            0x21 => 0x09, // F
            0x22 => 0x0A, // G
            0x23 => 0x0B, // H
            0x24 => 0x0D, // J
            0x25 => 0x0E, // K
            0x26 => 0x0F, // L
            0x27 => 0x33, // ;
            0x28 => 0x34, // '
            0x29 => 0x35, // ` (半角/全角 on JIS)
            0x2A => 0xE1, // left Shift
            0x2B => 0x31, // backslash
            0x2C => 0x1D, // Z
            0x2D => 0x1B, // X
            0x2E => 0x06, // C
            0x2F => 0x19, // V
            0x30 => 0x05, // B
            0x31 => 0x11, // N
            0x32 => 0x10, // M
            0x33 => 0x36, // ,
            0x34 => 0x37, // .
            0x35 => 0x38, // /
            0x36 => 0xE5, // right Shift
            0x37 => 0x55, // keypad *
            0x38 => 0xE2, // left Alt
            0x39 => 0x2C, // Space
            0x3A => 0x39, // Caps Lock (英数 on JIS)
            0x3B..=0x44 => 0x3A + (scan as u16 - 0x3B), // F1..F10
            0x46 => 0x47, // Scroll Lock
            0x47 => 0x5F, // keypad 7
            0x48 => 0x60, // keypad 8
            0x49 => 0x61, // keypad 9
            0x4A => 0x56, // keypad -
            0x4B => 0x5C, // keypad 4
            0x4C => 0x5D, // keypad 5
            0x4D => 0x5E, // keypad 6
            0x4E => 0x57, // keypad +
            0x4F => 0x59, // keypad 1
            0x50 => 0x5A, // keypad 2
            0x51 => 0x5B, // keypad 3
            0x52 => 0x62, // keypad 0
            0x53 => 0x63, // keypad .
            0x56 => 0x64, // ISO extra key
            0x57 => 0x44, // F11
            0x58 => 0x45, // F12
            0x59 => 0x67, // keypad =
            0x64..=0x6E => 0x68 + (scan as u16 - 0x64), // F13..F23
            0x70 => hid::INTL_KANA,
            0x73 => hid::INTL_RO,
            0x76 => 0x73, // F24
            0x79 => hid::INTL_HENKAN,
            0x7B => hid::INTL_MUHENKAN,
            0x7D => hid::INTL_YEN,
            0x7E => 0x85, // keypad , (JIS)
            _ => return None,
        }
    };
    Some(h)
}

/// Swap Ctrl and the Windows key so Windows-keyboard muscle memory works on the Mac:
/// Ctrl -> Cmd, Win -> Ctrl. Alt stays Alt (= Option).
pub fn swap_ctrl_cmd(h: u16) -> u16 {
    match h {
        hid::LEFT_CTRL => hid::LEFT_GUI,
        hid::LEFT_GUI => hid::LEFT_CTRL,
        hid::RIGHT_CTRL => hid::RIGHT_GUI,
        hid::RIGHT_GUI => hid::RIGHT_CTRL,
        other => other,
    }
}

pub mod mac {
    //! macOS virtual keycodes (kVK_* in Carbon's Events.h) that are referred to by name.
    pub const RETURN: u16 = 0x24;
    pub const COMMAND: u16 = 0x37;
    pub const SHIFT: u16 = 0x38;
    pub const CAPS_LOCK: u16 = 0x39;
    pub const OPTION: u16 = 0x3A;
    pub const CONTROL: u16 = 0x3B;
    pub const RIGHT_COMMAND: u16 = 0x36;
    pub const RIGHT_SHIFT: u16 = 0x3C;
    pub const RIGHT_OPTION: u16 = 0x3D;
    pub const RIGHT_CONTROL: u16 = 0x3E;
    pub const FUNCTION: u16 = 0x3F;
    pub const JIS_EISU: u16 = 0x66;
    pub const JIS_KANA: u16 = 0x68;
}

/// HID usage -> macOS virtual keycode.
pub fn hid_to_mac(h: u16) -> Option<u16> {
    let k = match h {
        0x04 => 0x00, // A
        0x05 => 0x0B, // B
        0x06 => 0x08, // C
        0x07 => 0x02, // D
        0x08 => 0x0E, // E
        0x09 => 0x03, // F
        0x0A => 0x05, // G
        0x0B => 0x04, // H
        0x0C => 0x22, // I
        0x0D => 0x26, // J
        0x0E => 0x28, // K
        0x0F => 0x25, // L
        0x10 => 0x2E, // M
        0x11 => 0x2D, // N
        0x12 => 0x1F, // O
        0x13 => 0x23, // P
        0x14 => 0x0C, // Q
        0x15 => 0x0F, // R
        0x16 => 0x01, // S
        0x17 => 0x11, // T
        0x18 => 0x20, // U
        0x19 => 0x09, // V
        0x1A => 0x0D, // W
        0x1B => 0x07, // X
        0x1C => 0x10, // Y
        0x1D => 0x06, // Z
        0x1E => 0x12, // 1
        0x1F => 0x13, // 2
        0x20 => 0x14, // 3
        0x21 => 0x15, // 4
        0x22 => 0x17, // 5
        0x23 => 0x16, // 6
        0x24 => 0x1A, // 7
        0x25 => 0x1C, // 8
        0x26 => 0x19, // 9
        0x27 => 0x1D, // 0
        0x28 => mac::RETURN,
        0x29 => 0x35, // Escape
        0x2A => 0x33, // Backspace (Mac "Delete")
        0x2B => 0x30, // Tab
        0x2C => 0x31, // Space
        0x2D => 0x1B, // -
        0x2E => 0x18, // =
        0x2F => 0x21, // [
        0x30 => 0x1E, // ]
        0x31 => 0x2A, // backslash
        0x32 => 0x2A, // non-US # (same physical spot)
        0x33 => 0x29, // ;
        0x34 => 0x27, // '
        0x35 => 0x32, // `
        0x36 => 0x2B, // ,
        0x37 => 0x2F, // .
        0x38 => 0x2C, // /
        0x39 => mac::CAPS_LOCK,
        0x3A => 0x7A, // F1
        0x3B => 0x78, // F2
        0x3C => 0x63, // F3
        0x3D => 0x76, // F4
        0x3E => 0x60, // F5
        0x3F => 0x61, // F6
        0x40 => 0x62, // F7
        0x41 => 0x64, // F8
        0x42 => 0x65, // F9
        0x43 => 0x6D, // F10
        0x44 => 0x67, // F11
        0x45 => 0x6F, // F12
        0x46 => 0x69, // Print Screen -> F13
        0x47 => 0x6B, // Scroll Lock  -> F14
        0x48 => 0x71, // Pause        -> F15
        0x49 => 0x72, // Insert       -> Help
        0x4A => 0x73, // Home
        0x4B => 0x74, // Page Up
        0x4C => 0x75, // Delete (forward delete)
        0x4D => 0x77, // End
        0x4E => 0x79, // Page Down
        0x4F => 0x7C, // Right
        0x50 => 0x7B, // Left
        0x51 => 0x7D, // Down
        0x52 => 0x7E, // Up
        0x53 => 0x47, // Num Lock -> keypad Clear
        0x54 => 0x4B, // keypad /
        0x55 => 0x43, // keypad *
        0x56 => 0x4E, // keypad -
        0x57 => 0x45, // keypad +
        0x58 => 0x4C, // keypad Enter
        0x59 => 0x53, // keypad 1
        0x5A => 0x54, // keypad 2
        0x5B => 0x55, // keypad 3
        0x5C => 0x56, // keypad 4
        0x5D => 0x57, // keypad 5
        0x5E => 0x58, // keypad 6
        0x5F => 0x59, // keypad 7
        0x60 => 0x5B, // keypad 8
        0x61 => 0x5C, // keypad 9
        0x62 => 0x52, // keypad 0
        0x63 => 0x41, // keypad .
        0x64 => 0x0A, // ISO section key
        0x67 => 0x51, // keypad =
        0x68 => 0x69, // F13
        0x69 => 0x6B, // F14
        0x6A => 0x71, // F15
        0x6B => 0x6A, // F16
        0x6C => 0x40, // F17
        0x6D => 0x4F, // F18
        0x6E => 0x50, // F19
        0x6F => 0x5A, // F20
        0x85 => 0x5F, // keypad , (JIS)
        hid::INTL_RO => 0x5E,     // JIS _
        hid::INTL_KANA => mac::JIS_KANA,
        hid::INTL_YEN => 0x5D,    // JIS ¥
        hid::INTL_HENKAN => mac::JIS_KANA,
        hid::INTL_MUHENKAN => mac::JIS_EISU,
        hid::LANG1 => mac::JIS_KANA,
        hid::LANG2 => mac::JIS_EISU,
        hid::LEFT_CTRL => mac::CONTROL,
        hid::LEFT_SHIFT => mac::SHIFT,
        hid::LEFT_ALT => mac::OPTION,
        hid::LEFT_GUI => mac::COMMAND,
        hid::RIGHT_CTRL => mac::RIGHT_CONTROL,
        hid::RIGHT_SHIFT => mac::RIGHT_SHIFT,
        hid::RIGHT_ALT => mac::RIGHT_OPTION,
        hid::RIGHT_GUI => mac::RIGHT_COMMAND,
        _ => return None,
    };
    Some(k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn letters_digits_end_to_end() {
        // scan code, expected mac keycode
        let cases = [
            (0x1E, 0x00), // A
            (0x10, 0x0C), // Q
            (0x2C, 0x06), // Z
            (0x32, 0x2E), // M
            (0x02, 0x12), // 1
            (0x0B, 0x1D), // 0
            (0x1C, 0x24), // Enter -> Return
            (0x0E, 0x33), // Backspace
        ];
        for (scan, want) in cases {
            let h = win_to_hid(scan, false, 0).unwrap();
            assert_eq!(hid_to_mac(h), Some(want), "scan 0x{scan:02x}");
        }
    }

    #[test]
    fn extended_keys() {
        assert_eq!(win_to_hid(0x48, true, 0), Some(0x52)); // Up arrow
        assert_eq!(win_to_hid(0x48, false, 0), Some(0x60)); // keypad 8
        assert_eq!(win_to_hid(0x1D, true, 0), Some(hid::RIGHT_CTRL));
        assert_eq!(win_to_hid(0x5B, true, 0), Some(hid::LEFT_GUI));
        assert_eq!(win_to_hid(0x45, false, VK_PAUSE), Some(0x48));
        assert_eq!(win_to_hid(0x45, true, VK_NUMLOCK), Some(0x53));
    }

    #[test]
    fn every_scan_code_maps_through_to_mac_except_known_gaps() {
        // Keys with no Mac equivalent are allowed to stop at HID.
        let no_mac = [0x65u16, 0x70, 0x71, 0x72, 0x73];
        for ext in [false, true] {
            for scan in 0..0x80u32 {
                if let Some(h) = win_to_hid(scan, ext, 0) {
                    if !no_mac.contains(&h) {
                        assert!(hid_to_mac(h).is_some(), "HID 0x{h:02x} has no Mac key");
                    }
                }
            }
        }
    }

    #[test]
    fn scan_table_has_no_accidental_duplicates() {
        let mut seen: HashMap<u16, (u32, bool)> = HashMap::new();
        for ext in [false, true] {
            for scan in 0..0x80u32 {
                if let Some(h) = win_to_hid(scan, ext, 0) {
                    // Right Shift is legitimately reported both ways.
                    if h == hid::RIGHT_SHIFT {
                        continue;
                    }
                    if let Some(prev) = seen.insert(h, (scan, ext)) {
                        panic!("HID 0x{h:02x} from both {prev:?} and {:?}", (scan, ext));
                    }
                }
            }
        }
    }

    #[test]
    fn ctrl_cmd_swap() {
        assert_eq!(swap_ctrl_cmd(hid::LEFT_CTRL), hid::LEFT_GUI);
        assert_eq!(swap_ctrl_cmd(hid::LEFT_GUI), hid::LEFT_CTRL);
        assert_eq!(swap_ctrl_cmd(hid::RIGHT_CTRL), hid::RIGHT_GUI);
        assert_eq!(swap_ctrl_cmd(hid::LEFT_ALT), hid::LEFT_ALT);
        assert_eq!(swap_ctrl_cmd(0x06), 0x06);
        // Ctrl+C on the Windows keyboard becomes Cmd+C.
        assert_eq!(hid_to_mac(swap_ctrl_cmd(hid::LEFT_CTRL)), Some(mac::COMMAND));
    }
}
