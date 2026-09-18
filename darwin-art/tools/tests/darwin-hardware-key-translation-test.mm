#import <AppKit/AppKit.h>

#include "compat/input/darwin_hardware_key_translation.h"

#include <array>
#include <cstdint>
#include <cstdio>

namespace {

using darwin_art::input::AndroidKeyCode;
using darwin_art::input::AndroidMetaState;
using darwin_art::input::AndroidScanCode;
using darwin_art::input::ModifierFlagForKey;

std::array<uint32_t, 65536> ExpectedKeyCodes() {
  std::array<uint32_t, 65536> expected{};
  // Independent AppKit virtual-key table (not derived from the provider).
  const uint32_t values[][2] = {
      {0,29},{1,47},{2,32},{3,34},{4,36},{5,35},{6,54},{7,52},{8,31},
      {9,50},{11,30},{12,45},{13,51},{14,33},{15,46},{16,53},{17,48},
      {18,8},{19,9},{20,10},{21,11},{22,13},{23,12},{24,70},{25,16},
      {26,14},{27,69},{28,15},{29,7},{30,72},{31,43},{32,49},{33,71},
      {34,37},{35,44},{36,66},{37,40},{38,38},{39,75},{40,39},{41,74},
      {42,73},{43,55},{44,76},{45,42},{46,41},{47,56},{48,61},{49,62},
      {50,68},{51,67},{53,111},{54,118},{55,117},{56,59},{57,115},
      {58,57},{59,113},{60,60},{61,58},{62,114},{63,119},{123,21},
      {124,22},{125,20},{126,19},
  };
  for (const auto& value : values) expected[value[0]] = value[1];
  return expected;
}

std::array<NSEventModifierFlags, 65536> ExpectedModifierFlags() {
  std::array<NSEventModifierFlags, 65536> expected{};
  expected[54] = NSEventModifierFlagCommand;
  expected[55] = NSEventModifierFlagCommand;
  expected[56] = NSEventModifierFlagShift;
  expected[60] = NSEventModifierFlagShift;
  expected[57] = NSEventModifierFlagCapsLock;
  expected[58] = NSEventModifierFlagOption;
  expected[61] = NSEventModifierFlagOption;
  expected[59] = NSEventModifierFlagControl;
  expected[62] = NSEventModifierFlagControl;
  expected[63] = NSEventModifierFlagFunction;
  return expected;
}

bool Check(bool value, const char* message, unsigned value_id = 0) {
  if (value) return true;
  std::fprintf(stderr, "hardware key translation failure: %s (%u)\n", message, value_id);
  return false;
}

bool CheckScanCodes() {
  std::array<uint32_t, 128> expected{};
  expected[7] = 11;
  for (uint32_t key = 8; key <= 16; ++key) expected[key] = key - 6;
  const uint32_t values[][2] = {
      {29,30},{30,48},{31,46},{32,32},{33,18},{34,33},{35,34},{36,35},
      {37,23},{38,36},{39,37},{40,38},{41,50},{42,49},{43,24},{44,25},
      {45,16},{46,19},{47,31},{48,20},{49,22},{50,47},{51,17},{52,45},
      {53,21},{54,44},{55,51},{56,52},{62,57},{66,28},{67,14},{68,41},
      {69,12},{70,13},{71,26},{72,27},{73,43},{74,39},{75,40},{76,53},
      {111,1},
  };
  for (const auto& value : values) expected[value[0]] = value[1];
  for (uint32_t key = 0; key < expected.size(); ++key) {
    if (!Check(AndroidScanCode(key) == expected[key], "scan code", key)) return false;
  }
  // TAB and modifier Android key codes intentionally have no evdev mapping.
  for (uint32_t key : {57u, 58u, 59u, 60u, 61u, 63u, 127u}) {
    if (!Check(AndroidScanCode(key) == 0, "unmapped scan code", key)) return false;
  }
  if (!Check(AndroidScanCode(UINT32_MAX) == 0, "out-of-range scan code")) return false;
  return true;
}

}  // namespace

int main() {
  const auto expected_keys = ExpectedKeyCodes();
  const auto expected_modifiers = ExpectedModifierFlags();
  for (uint32_t code = 0; code <= 0xffffu; ++code) {
    if (!Check(AndroidKeyCode(static_cast<unsigned short>(code)) == expected_keys[code],
              "key code", code)) return 1;
    if (!Check(ModifierFlagForKey(static_cast<unsigned short>(code)) == expected_modifiers[code],
              "modifier key", code)) return 1;
  }
  if (!Check(AndroidMetaState(NSEventModifierFlagShift | NSEventModifierFlagOption |
                                  NSEventModifierFlagFunction | NSEventModifierFlagControl |
                                  NSEventModifierFlagCommand | NSEventModifierFlagCapsLock)
                 == 0x11100b,
             "combined modifier state")) return 1;
  if (!Check(AndroidMetaState(0) == 0, "empty modifier state")) return 1;
  const NSEventModifierFlags modifier_flags[] = {
      NSEventModifierFlagShift, NSEventModifierFlagOption,
      NSEventModifierFlagFunction, NSEventModifierFlagControl,
      NSEventModifierFlagCommand, NSEventModifierFlagCapsLock,
  };
  const uint32_t modifier_bits[] = {0x1, 0x2, 0x8, 0x1000, 0x10000, 0x100000};
  for (unsigned mask = 0; mask < 64; ++mask) {
    NSEventModifierFlags flags = 0;
    uint32_t expected = 0;
    for (unsigned bit = 0; bit < 6; ++bit) {
      if ((mask & (1u << bit)) != 0) {
        flags |= modifier_flags[bit];
        expected |= modifier_bits[bit];
      }
    }
    if (!Check(AndroidMetaState(flags) == expected, "modifier combination", mask)) return 1;
  }
  if (!Check(AndroidMetaState(NSEventModifierFlagNumericPad |
                                  NSEventModifierFlagHelp) == 0,
             "irrelevant modifier flags")) return 1;
  if (!Check(CheckScanCodes(), "scan code table")) return 1;
  std::puts("darwin hardware key translation checks passed");
  return 0;
}
