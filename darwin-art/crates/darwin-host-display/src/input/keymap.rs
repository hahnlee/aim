//! macOS virtual key codes (`kVK_*`, `HIToolbox/Events.h`) to Linux key
//! codes (`KEY_*`).
//!
//! Keys are physical positions: the ANSI, ISO and JIS keys map to the keys
//! at the same place on a PC keyboard, as Linux's HID driver maps an Apple
//! keyboard. Characters are Android's business (its key layout and
//! character map, `Generic.kl` and `Generic.kcm`).

/// `(kVK, KEY)` pairs.
const MAP: &[(u16, u16)] = &[
    (0x00, 30),  // A
    (0x01, 31),  // S
    (0x02, 32),  // D
    (0x03, 33),  // F
    (0x04, 35),  // H
    (0x05, 34),  // G
    (0x06, 44),  // Z
    (0x07, 45),  // X
    (0x08, 46),  // C
    (0x09, 47),  // V
    (0x0a, 86),  // ISO section: KEY_102ND
    (0x0b, 48),  // B
    (0x0c, 16),  // Q
    (0x0d, 17),  // W
    (0x0e, 18),  // E
    (0x0f, 19),  // R
    (0x10, 21),  // Y
    (0x11, 20),  // T
    (0x12, 2),   // 1
    (0x13, 3),   // 2
    (0x14, 4),   // 3
    (0x15, 5),   // 4
    (0x16, 7),   // 6
    (0x17, 6),   // 5
    (0x18, 13),  // =
    (0x19, 10),  // 9
    (0x1a, 8),   // 7
    (0x1b, 12),  // -
    (0x1c, 9),   // 8
    (0x1d, 11),  // 0
    (0x1e, 27),  // ]
    (0x1f, 24),  // O
    (0x20, 22),  // U
    (0x21, 26),  // [
    (0x22, 23),  // I
    (0x23, 25),  // P
    (0x24, 28),  // Return: KEY_ENTER
    (0x25, 38),  // L
    (0x26, 36),  // J
    (0x27, 40),  // '
    (0x28, 37),  // K
    (0x29, 39),  // ;
    (0x2a, 43),  // backslash
    (0x2b, 51),  // ,
    (0x2c, 53),  // /
    (0x2d, 49),  // N
    (0x2e, 50),  // M
    (0x2f, 52),  // .
    (0x30, 15),  // Tab
    (0x31, 57),  // Space
    (0x32, 41),  // `
    (0x33, 14),  // Delete: KEY_BACKSPACE
    (0x35, 1),   // Escape
    (0x36, 126), // right Command: KEY_RIGHTMETA
    (0x37, 125), // Command: KEY_LEFTMETA
    (0x38, 42),  // Shift
    (0x39, 58),  // Caps Lock
    (0x3a, 56),  // Option: KEY_LEFTALT
    (0x3b, 29),  // Control
    (0x3c, 54),  // right Shift
    (0x3d, 100), // right Option: KEY_RIGHTALT
    (0x3e, 97),  // right Control
    (0x3f, super::codes::KEY_FN),
    (0x40, 187), // F17
    (0x41, 83),  // keypad .
    (0x43, 55),  // keypad *
    (0x45, 78),  // keypad +
    (0x47, 69),  // keypad Clear, at Num Lock's place
    (0x48, 115), // volume up
    (0x49, 114), // volume down
    (0x4a, 113), // mute
    (0x4b, 98),  // keypad /
    (0x4c, 96),  // keypad Enter
    (0x4e, 74),  // keypad -
    (0x4f, 188), // F18
    (0x50, 189), // F19
    (0x51, 117), // keypad =
    (0x52, 82),  // keypad 0
    (0x53, 79),  // keypad 1
    (0x54, 80),  // keypad 2
    (0x55, 81),  // keypad 3
    (0x56, 75),  // keypad 4
    (0x57, 76),  // keypad 5
    (0x58, 77),  // keypad 6
    (0x59, 71),  // keypad 7
    (0x5a, 190), // F20
    (0x5b, 72),  // keypad 8
    (0x5c, 73),  // keypad 9
    (0x5d, 124), // JIS Yen: KEY_YEN
    (0x5e, 89),  // JIS underscore: KEY_RO
    (0x5f, 95),  // JIS keypad comma: KEY_KPJPCOMMA
    (0x60, 63),  // F5
    (0x61, 64),  // F6
    (0x62, 65),  // F7
    (0x63, 61),  // F3
    (0x64, 66),  // F8
    (0x65, 67),  // F9
    (0x66, 123), // JIS Eisu (HID LANG2): KEY_HANJA, EISU in Generic.kl
    (0x67, 87),  // F11
    (0x68, 122), // JIS Kana (HID LANG1): KEY_HANGEUL, KANA in Generic.kl
    (0x69, 183), // F13
    (0x6a, 186), // F16
    (0x6b, 184), // F14
    (0x6d, 68),  // F10
    (0x6e, 127), // context menu: KEY_COMPOSE
    (0x6f, 88),  // F12
    (0x71, 185), // F15
    (0x72, 110), // Help, at Insert's place: KEY_INSERT
    (0x73, 102), // Home
    (0x74, 104), // Page Up
    (0x75, 111), // Forward Delete: KEY_DELETE
    (0x76, 62),  // F4
    (0x77, 107), // End
    (0x78, 60),  // F2
    (0x79, 109), // Page Down
    (0x7a, 59),  // F1
    (0x7b, 105), // Left
    (0x7c, 106), // Right
    (0x7d, 108), // Down
    (0x7e, 103), // Up
];

/// The Linux key at macOS virtual key code `mac`.
pub fn linux_key(mac: u16) -> Option<u16> {
    MAP.iter().find(|(m, _)| *m == mac).map(|(_, l)| *l)
}

/// Every Linux key the keyboard has.
pub fn linux_keys() -> impl Iterator<Item = u16> {
    MAP.iter().map(|(_, l)| *l)
}

// NSEventModifierFlags and the device-dependent bits (IOLLEvent.h
// NX_DEVICE*KEYMASK) that tell left from right.
const FUNCTION: u64 = 1 << 23;
const LEFT_CONTROL: u64 = 0x0001;
const LEFT_SHIFT: u64 = 0x0002;
const RIGHT_SHIFT: u64 = 0x0004;
const LEFT_COMMAND: u64 = 0x0008;
const RIGHT_COMMAND: u64 = 0x0010;
const LEFT_OPTION: u64 = 0x0020;
const RIGHT_OPTION: u64 = 0x0040;
const RIGHT_CONTROL: u64 = 0x2000;

/// What a `flagsChanged:` event for key `mac` means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    /// The key is now down (true) or up.
    Held(bool),
    /// Caps Lock: AppKit reports the lock's state changing, once per press,
    /// so the key goes down and up.
    Toggled,
}

/// The modifier key `mac` of a `flagsChanged:` event with `flags`
/// (`NSEvent.modifierFlags`); None for other keys.
pub fn modifier(mac: u16, flags: u64) -> Option<Modifier> {
    let mask = match mac {
        0x39 => return Some(Modifier::Toggled),
        0x38 => LEFT_SHIFT,
        0x3c => RIGHT_SHIFT,
        0x3b => LEFT_CONTROL,
        0x3e => RIGHT_CONTROL,
        0x3a => LEFT_OPTION,
        0x3d => RIGHT_OPTION,
        0x37 => LEFT_COMMAND,
        0x36 => RIGHT_COMMAND,
        0x3f => FUNCTION,
        _ => return None,
    };
    Some(Modifier::Held(flags & mask != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_physical_positions() {
        // QWERTY row, then the ones Apple places differently.
        let row: Vec<u16> = [0x0c, 0x0d, 0x0e, 0x0f, 0x11, 0x10]
            .iter()
            .map(|&k| linux_key(k).unwrap())
            .collect();
        assert_eq!(row, [16, 17, 18, 19, 20, 21]); // KEY_Q..KEY_Y
        assert_eq!(linux_key(0x33), Some(14)); // Delete is Backspace
        assert_eq!(linux_key(0x75), Some(111)); // Forward Delete
        assert_eq!(linux_key(0x37), Some(125)); // Command is Meta
        assert_eq!(linux_key(0x3a), Some(56)); // Option is Alt
        assert_eq!(linux_key(0x34), None);
    }

    #[test]
    fn every_key_once() {
        let mut macs: Vec<u16> = MAP.iter().map(|(m, _)| *m).collect();
        let mut linux: Vec<u16> = linux_keys().collect();
        let n = macs.len();
        macs.sort();
        macs.dedup();
        linux.sort();
        linux.dedup();
        assert_eq!((macs.len(), linux.len()), (n, n));
    }

    #[test]
    fn modifiers_tell_left_from_right() {
        // Right Shift down while left is up: the shift flag and the right
        // device bit.
        let flags = (1 << 17) | RIGHT_SHIFT;
        assert_eq!(modifier(0x3c, flags), Some(Modifier::Held(true)));
        assert_eq!(modifier(0x38, flags), Some(Modifier::Held(false)));
        assert_eq!(modifier(0x39, 0), Some(Modifier::Toggled));
        assert_eq!(modifier(0x00, flags), None);
    }
}
