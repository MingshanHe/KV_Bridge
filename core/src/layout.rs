//! What a Windows keyboard *types*, per physical key.
//!
//! Keys travel as physical positions (HID usages). When the Windows keyboard and the
//! Mac use different layouts (e.g. a UK keyboard on Windows and a Japanese JIS Mac),
//! the same position means a different symbol on each side. The Mac uses these tables
//! to find the character the user meant, then presses whichever Mac key produces it.
//!
//! Only symbol and digit keys are listed: letters sit in the same place on all of
//! these layouts, and leaving them alone keeps Japanese (romaji) input natural.

/// A Windows keyboard layout we know the symbols of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinLayout {
    Us,
    Uk,
}

impl WinLayout {
    /// From a Windows HKL (as returned by GetKeyboardLayout). The high word is the
    /// keyboard layout device (0x0809 = United Kingdom, 0x0409 = US, ...); the low
    /// word is the input language. Variant layouts (high nibble 0xF) fall back to the
    /// language.
    pub fn from_hkl(hkl: u32) -> Option<WinLayout> {
        let device = (hkl >> 16) as u16;
        let lang = hkl as u16;
        let id = if device & 0xF000 == 0xF000 || device == 0 { lang } else { device };
        match id {
            0x0809 | 0x1809 => Some(WinLayout::Uk), // United Kingdom, Ireland
            0x0409 => Some(WinLayout::Us),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            WinLayout::Us => "US",
            WinLayout::Uk => "UK",
        }
    }

    /// The character typed by HID key `hid`, with or without Shift.
    /// `None` for keys not in the table (letters, keypad, control keys ...).
    pub fn char_for(self, hid: u16, shift: bool) -> Option<char> {
        let (plain, shifted) = match (self, hid) {
            (_, 0x1E) => ('1', '!'),
            (WinLayout::Us, 0x1F) => ('2', '@'),
            (WinLayout::Uk, 0x1F) => ('2', '"'),
            (WinLayout::Us, 0x20) => ('3', '#'),
            (WinLayout::Uk, 0x20) => ('3', '£'),
            (_, 0x21) => ('4', '$'),
            (_, 0x22) => ('5', '%'),
            (_, 0x23) => ('6', '^'),
            (_, 0x24) => ('7', '&'),
            (_, 0x25) => ('8', '*'),
            (_, 0x26) => ('9', '('),
            (_, 0x27) => ('0', ')'),
            (_, 0x2D) => ('-', '_'),
            (_, 0x2E) => ('=', '+'),
            (_, 0x2F) => ('[', '{'),
            (_, 0x30) => (']', '}'),
            // Windows reports the key left of Enter as scan 0x2B -> HID 0x31.
            (WinLayout::Us, 0x31) => ('\\', '|'),
            (WinLayout::Uk, 0x31) => ('#', '~'),
            (_, 0x33) => (';', ':'),
            (WinLayout::Us, 0x34) => ('\'', '"'),
            (WinLayout::Uk, 0x34) => ('\'', '@'),
            (WinLayout::Us, 0x35) => ('`', '~'),
            (WinLayout::Uk, 0x35) => ('`', '¬'),
            (_, 0x36) => (',', '<'),
            (_, 0x37) => ('.', '>'),
            (_, 0x38) => ('/', '?'),
            // Extra ISO key left of Z.
            (WinLayout::Uk, 0x64) => ('\\', '|'),
            _ => return None,
        };
        Some(if shift { shifted } else { plain })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkl_detection() {
        assert_eq!(WinLayout::from_hkl(0x0809_0809), Some(WinLayout::Uk));
        // English (US) language with the UK keyboard layout.
        assert_eq!(WinLayout::from_hkl(0x0809_0409), Some(WinLayout::Uk));
        assert_eq!(WinLayout::from_hkl(0x0409_0409), Some(WinLayout::Us));
        // Variant layout id -> use the language.
        assert_eq!(WinLayout::from_hkl(0xF002_0409), Some(WinLayout::Us));
        assert_eq!(WinLayout::from_hkl(0x0411_0411), None); // Japanese
    }

    #[test]
    fn uk_symbols() {
        let uk = WinLayout::Uk;
        assert_eq!(uk.char_for(0x2D, true), Some('_'));
        assert_eq!(uk.char_for(0x33, true), Some(':'));
        assert_eq!(uk.char_for(0x1F, true), Some('"'));
        assert_eq!(uk.char_for(0x34, true), Some('@'));
        assert_eq!(uk.char_for(0x31, false), Some('#'));
        assert_eq!(uk.char_for(0x64, false), Some('\\'));
        assert_eq!(uk.char_for(0x20, true), Some('£'));
        assert_eq!(uk.char_for(0x04, false), None); // letters untouched
    }
}
