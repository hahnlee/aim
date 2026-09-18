#import "darwin_hardware_key_translation.h"

namespace darwin_art::input {

uint32_t AndroidMetaState(NSEventModifierFlags flags) {
  uint32_t meta = 0;
  if ((flags & NSEventModifierFlagShift) != 0) meta |= 0x1;
  if ((flags & NSEventModifierFlagOption) != 0) meta |= 0x2;
  if ((flags & NSEventModifierFlagFunction) != 0) meta |= 0x8;
  if ((flags & NSEventModifierFlagControl) != 0) meta |= 0x1000;
  if ((flags & NSEventModifierFlagCommand) != 0) meta |= 0x10000;
  if ((flags & NSEventModifierFlagCapsLock) != 0) meta |= 0x100000;
  return meta;
}

uint32_t AndroidKeyCode(unsigned short code) {
  switch (code) {
    case 0: return 29;  // A
    case 1: return 47;  // S
    case 2: return 32;  // D
    case 3: return 34;  // F
    case 4: return 36;  // H
    case 5: return 35;  // G
    case 6: return 54;  // Z
    case 7: return 52;  // X
    case 8: return 31;  // C
    case 9: return 50;  // V
    case 11: return 30; // B
    case 12: return 45; // Q
    case 13: return 51; // W
    case 14: return 33; // E
    case 15: return 46; // R
    case 16: return 53; // Y
    case 17: return 48; // T
    case 18: return 8;  // 1
    case 19: return 9;  // 2
    case 20: return 10; // 3
    case 21: return 11; // 4
    case 22: return 13; // 6
    case 23: return 12; // 5
    case 24: return 70; // =
    case 25: return 16; // 9
    case 26: return 14; // 7
    case 27: return 69; // -
    case 28: return 15; // 8
    case 29: return 7;  // 0
    case 30: return 72; // ]
    case 31: return 43; // O
    case 32: return 49; // U
    case 33: return 71; // [
    case 34: return 37; // I
    case 35: return 44; // P
    case 36: return 66; // ENTER
    case 37: return 40; // L
    case 38: return 38; // J
    case 39: return 75; // '
    case 40: return 39; // K
    case 41: return 74; // ;
    case 42: return 73; // backslash
    case 43: return 55; // ,
    case 44: return 76; // /
    case 45: return 42; // N
    case 46: return 41; // M
    case 47: return 56; // .
    case 48: return 61; // TAB
    case 49: return 62; // SPACE
    case 50: return 68; // grave
    case 51: return 67; // DEL
    case 53: return 111; // ESCAPE
    case 54: return 118; // META_RIGHT
    case 55: return 117; // META_LEFT
    case 56: return 59;  // SHIFT_LEFT
    case 57: return 115; // CAPS_LOCK
    case 58: return 57;  // ALT_LEFT
    case 59: return 113; // CTRL_LEFT
    case 60: return 60;  // SHIFT_RIGHT
    case 61: return 58;  // ALT_RIGHT
    case 62: return 114; // CTRL_RIGHT
    case 63: return 119; // FUNCTION
    case 123: return 21; // DPAD_LEFT
    case 124: return 22; // DPAD_RIGHT
    case 125: return 20; // DPAD_DOWN
    case 126: return 19; // DPAD_UP
    default: return 0;
  }
}

uint32_t AndroidScanCode(uint32_t key_code) {
  // Linux evdev scan codes carried by Android's native InputDispatcher.
  // AppKit virtual key codes are a different namespace and must not leak into
  // KeyEvent.getScanCode().
  if (key_code == 7) return 11;
  if (key_code >= 8 && key_code <= 16) return key_code - 6;
  switch (key_code) {
    case 29: return 30; case 30: return 48; case 31: return 46;
    case 32: return 32; case 33: return 18; case 34: return 33;
    case 35: return 34; case 36: return 35; case 37: return 23;
    case 38: return 36; case 39: return 37; case 40: return 38;
    case 41: return 50; case 42: return 49; case 43: return 24;
    case 44: return 25; case 45: return 16; case 46: return 19;
    case 47: return 31; case 48: return 20; case 49: return 22;
    case 50: return 47; case 51: return 17; case 52: return 45;
    case 53: return 21; case 54: return 44; case 55: return 51;
    case 56: return 52; case 62: return 57; case 66: return 28;
    case 67: return 14; case 68: return 41; case 69: return 12;
    case 70: return 13; case 71: return 26; case 72: return 27;
    case 73: return 43; case 74: return 39; case 75: return 40;
    case 76: return 53; case 111: return 1;
    default: return 0;
  }
}

NSEventModifierFlags ModifierFlagForKey(unsigned short code) {
  switch (code) {
    case 54:
    case 55: return NSEventModifierFlagCommand;
    case 56:
    case 60: return NSEventModifierFlagShift;
    case 57: return NSEventModifierFlagCapsLock;
    case 58:
    case 61: return NSEventModifierFlagOption;
    case 59:
    case 62: return NSEventModifierFlagControl;
    case 63: return NSEventModifierFlagFunction;
    default: return 0;
  }
}

}  // namespace darwin_art::input
