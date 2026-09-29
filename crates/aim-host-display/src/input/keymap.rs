//! macOS virtual key codes (`kVK_*`, `HIToolbox/Events.h`) to HID usages
//! and Linux key codes (`KEY_*`).
//!
//! Keys are physical positions: the ANSI, ISO and JIS keys map to the keys
//! at the same place on a PC keyboard, as Linux's HID driver maps an Apple
//! keyboard. Characters are Android's business (its key layout and
//! character map, `Generic.kl` and `Generic.kcm`).
//!
//! The usage (page << 16 | id, the HID Usage Tables' keyboard page 7; Fn is
//! Apple's top case page 0xff) is the key's scan code, as hid-input reports
//! it: the keyboard's keymap goes from it to the `KEY_*` it sends
//! (`EVIOCGKEYCODE`), starting from this table.

/// On an ISO keyboard, the two keys whose places differ from a PC's: the
/// key left of 1 (`kVK_ISO_Section`, usage 0x64) is PC's `KEY_GRAVE`, and
/// the key right of left Shift (`kVK_ANSI_Grave` there, usage 0x35) is
/// `KEY_102ND`. Linux's hid-apple swaps the same two (`iso_layout`).
pub const ISO: [(u32, u16); 2] = [(0x0007_0064, 41), (0x0007_0035, 86)];

/// The consumer page's AC Back, Back's usage.
pub const AC_BACK: u32 = 0x000c_0224;

/// `(kVK, usage, KEY)`.
const MAP: &[(u16, u32, u16)] = &[
    (0x00, 0x0007_0004, 30),  // A
    (0x01, 0x0007_0016, 31),  // S
    (0x02, 0x0007_0007, 32),  // D
    (0x03, 0x0007_0009, 33),  // F
    (0x04, 0x0007_000b, 35),  // H
    (0x05, 0x0007_000a, 34),  // G
    (0x06, 0x0007_001d, 44),  // Z
    (0x07, 0x0007_001b, 45),  // X
    (0x08, 0x0007_0006, 46),  // C
    (0x09, 0x0007_0019, 47),  // V
    (0x0a, 0x0007_0064, 86),  // ISO section: KEY_102ND
    (0x0b, 0x0007_0005, 48),  // B
    (0x0c, 0x0007_0014, 16),  // Q
    (0x0d, 0x0007_001a, 17),  // W
    (0x0e, 0x0007_0008, 18),  // E
    (0x0f, 0x0007_0015, 19),  // R
    (0x10, 0x0007_001c, 21),  // Y
    (0x11, 0x0007_0017, 20),  // T
    (0x12, 0x0007_001e, 2),   // 1
    (0x13, 0x0007_001f, 3),   // 2
    (0x14, 0x0007_0020, 4),   // 3
    (0x15, 0x0007_0021, 5),   // 4
    (0x16, 0x0007_0023, 7),   // 6
    (0x17, 0x0007_0022, 6),   // 5
    (0x18, 0x0007_002e, 13),  // =
    (0x19, 0x0007_0026, 10),  // 9
    (0x1a, 0x0007_0024, 8),   // 7
    (0x1b, 0x0007_002d, 12),  // -
    (0x1c, 0x0007_0025, 9),   // 8
    (0x1d, 0x0007_0027, 11),  // 0
    (0x1e, 0x0007_0030, 27),  // ]
    (0x1f, 0x0007_0012, 24),  // O
    (0x20, 0x0007_0018, 22),  // U
    (0x21, 0x0007_002f, 26),  // [
    (0x22, 0x0007_000c, 23),  // I
    (0x23, 0x0007_0013, 25),  // P
    (0x24, 0x0007_0028, 28),  // Return: KEY_ENTER
    (0x25, 0x0007_000f, 38),  // L
    (0x26, 0x0007_000d, 36),  // J
    (0x27, 0x0007_0034, 40),  // '
    (0x28, 0x0007_000e, 37),  // K
    (0x29, 0x0007_0033, 39),  // ;
    (0x2a, 0x0007_0031, 43),  // backslash
    (0x2b, 0x0007_0036, 51),  // ,
    (0x2c, 0x0007_0038, 53),  // /
    (0x2d, 0x0007_0011, 49),  // N
    (0x2e, 0x0007_0010, 50),  // M
    (0x2f, 0x0007_0037, 52),  // .
    (0x30, 0x0007_002b, 15),  // Tab
    (0x31, 0x0007_002c, 57),  // Space
    (0x32, 0x0007_0035, 41),  // `
    (0x33, 0x0007_002a, 14),  // Delete: KEY_BACKSPACE
    (0x35, 0x0007_0029, 1),   // Escape
    (0x36, 0x0007_00e7, 126), // right Command: KEY_RIGHTMETA
    (0x37, 0x0007_00e3, 125), // Command: KEY_LEFTMETA
    (0x38, 0x0007_00e1, 42),  // Shift
    (0x39, 0x0007_0039, 58),  // Caps Lock
    (0x3a, 0x0007_00e2, 56),  // Option: KEY_LEFTALT
    (0x3b, 0x0007_00e0, 29),  // Control
    (0x3c, 0x0007_00e5, 54),  // right Shift
    (0x3d, 0x0007_00e6, 100), // right Option: KEY_RIGHTALT
    (0x3e, 0x0007_00e4, 97),  // right Control
    (0x3f, 0x00ff_0003, super::codes::KEY_FN),
    (0x40, 0x0007_006c, 187), // F17
    (0x41, 0x0007_0063, 83),  // keypad .
    (0x43, 0x0007_0055, 55),  // keypad *
    (0x45, 0x0007_0057, 78),  // keypad +
    (0x47, 0x0007_0053, 69),  // keypad Clear, at Num Lock's place
    (0x48, 0x0007_0080, 115), // volume up
    (0x49, 0x0007_0081, 114), // volume down
    (0x4a, 0x0007_007f, 113), // mute
    (0x4b, 0x0007_0054, 98),  // keypad /
    (0x4c, 0x0007_0058, 96),  // keypad Enter
    (0x4e, 0x0007_0056, 74),  // keypad -
    (0x4f, 0x0007_006d, 188), // F18
    (0x50, 0x0007_006e, 189), // F19
    (0x51, 0x0007_0067, 117), // keypad =
    (0x52, 0x0007_0062, 82),  // keypad 0
    (0x53, 0x0007_0059, 79),  // keypad 1
    (0x54, 0x0007_005a, 80),  // keypad 2
    (0x55, 0x0007_005b, 81),  // keypad 3
    (0x56, 0x0007_005c, 75),  // keypad 4
    (0x57, 0x0007_005d, 76),  // keypad 5
    (0x58, 0x0007_005e, 77),  // keypad 6
    (0x59, 0x0007_005f, 71),  // keypad 7
    (0x5a, 0x0007_006f, 190), // F20
    (0x5b, 0x0007_0060, 72),  // keypad 8
    (0x5c, 0x0007_0061, 73),  // keypad 9
    (0x5d, 0x0007_0089, 124), // JIS Yen: KEY_YEN
    (0x5e, 0x0007_0087, 89),  // JIS underscore: KEY_RO
    (0x5f, 0x0007_0085, 95),  // JIS keypad comma: KEY_KPJPCOMMA
    (0x60, 0x0007_003e, 63),  // F5
    (0x61, 0x0007_003f, 64),  // F6
    (0x62, 0x0007_0040, 65),  // F7
    (0x63, 0x0007_003c, 61),  // F3
    (0x64, 0x0007_0041, 66),  // F8
    (0x65, 0x0007_0042, 67),  // F9
    (0x66, 0x0007_0091, 123), // JIS Eisu (HID LANG2): KEY_HANJA, EISU in Generic.kl
    (0x67, 0x0007_0044, 87),  // F11
    (0x68, 0x0007_0090, 122), // JIS Kana (HID LANG1): KEY_HANGEUL, KANA in Generic.kl
    (0x69, 0x0007_0068, 183), // F13
    (0x6a, 0x0007_006b, 186), // F16
    (0x6b, 0x0007_0069, 184), // F14
    (0x6d, 0x0007_0043, 68),  // F10
    (0x6e, 0x0007_0065, 127), // context menu: KEY_COMPOSE
    (0x6f, 0x0007_0045, 88),  // F12
    (0x71, 0x0007_006a, 185), // F15
    (0x72, 0x0007_0049, 110), // Help, at Insert's place: KEY_INSERT
    (0x73, 0x0007_004a, 102), // Home
    (0x74, 0x0007_004b, 104), // Page Up
    (0x75, 0x0007_004c, 111), // Forward Delete: KEY_DELETE
    (0x76, 0x0007_003d, 62),  // F4
    (0x77, 0x0007_004d, 107), // End
    (0x78, 0x0007_003b, 60),  // F2
    (0x79, 0x0007_004e, 109), // Page Down
    (0x7a, 0x0007_003a, 59),  // F1
    (0x7b, 0x0007_0050, 105), // Left
    (0x7c, 0x0007_004f, 106), // Right
    (0x7d, 0x0007_0051, 108), // Down
    (0x7e, 0x0007_0052, 103), // Up
];

/// The Linux key at macOS virtual key code `mac`, in the default keymap.
pub fn linux_key(mac: u16) -> Option<u16> {
    MAP.iter().find(|e| e.0 == mac).map(|e| e.2)
}

/// The usage (scan code) of macOS virtual key code `mac`.
pub fn usage(mac: u16) -> Option<u32> {
    MAP.iter().find(|e| e.0 == mac).map(|e| e.1)
}

/// The default keymap: `(usage, KEY)` in the table's order.
pub fn keymap() -> Vec<(u32, u16)> {
    MAP.iter().map(|e| (e.1, e.2)).collect()
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
        let mut macs: Vec<u16> = MAP.iter().map(|e| e.0).collect();
        let mut usages: Vec<u32> = MAP.iter().map(|e| e.1).collect();
        let mut linux: Vec<u16> = MAP.iter().map(|e| e.2).collect();
        let n = macs.len();
        macs.sort();
        macs.dedup();
        usages.sort();
        usages.dedup();
        linux.sort();
        linux.dedup();
        assert_eq!((macs.len(), usages.len(), linux.len()), (n, n, n));
        // A, Return, left GUI (Command), Fn.
        assert_eq!(usage(0x00), Some(0x0007_0004));
        assert_eq!(usage(0x24), Some(0x0007_0028));
        assert_eq!(usage(0x37), Some(0x0007_00e3));
        assert_eq!(usage(0x3f), Some(0x00ff_0003));
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
