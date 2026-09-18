#pragma once

#import <AppKit/AppKit.h>

#include <cstdint>

namespace darwin_art::input {

// AppKit hardware-key translation only; this provider owns no window, focus,
// input-state, or event-routing policy.
uint32_t AndroidMetaState(NSEventModifierFlags flags);
uint32_t AndroidKeyCode(unsigned short code);
uint32_t AndroidScanCode(uint32_t key_code);
NSEventModifierFlags ModifierFlagForKey(unsigned short code);

}  // namespace darwin_art::input
