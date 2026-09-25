#include "power_state_platform.h"

#include <CoreGraphics/CGDirectDisplay.h>
#include <CoreGraphics/CGDisplayConfiguration.h>
#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/ps/IOPSKeys.h>
#include <IOKit/ps/IOPowerSources.h>

namespace darwin_art::framework::power {

bool PlatformIsInteractive() {
  const CGDirectDisplayID display = CGMainDisplayID();
  return display != kCGNullDirectDisplay && !CGDisplayIsAsleep(display);
}

namespace {

int IntValue(CFDictionaryRef description, CFStringRef key, int fallback) {
  const auto number = static_cast<CFNumberRef>(CFDictionaryGetValue(description, key));
  int value = fallback;
  if (number == nullptr || CFGetTypeID(number) != CFNumberGetTypeID() ||
      !CFNumberGetValue(number, kCFNumberIntType, &value)) {
    return fallback;
  }
  return value;
}

bool BoolValue(CFDictionaryRef description, CFStringRef key) {
  const auto value = static_cast<CFBooleanRef>(CFDictionaryGetValue(description, key));
  return value != nullptr && CFGetTypeID(value) == CFBooleanGetTypeID() &&
         CFBooleanGetValue(value);
}

bool StringEquals(CFDictionaryRef description, CFStringRef key, CFStringRef expected) {
  const auto value = static_cast<CFStringRef>(CFDictionaryGetValue(description, key));
  return value != nullptr && CFGetTypeID(value) == CFStringGetTypeID() &&
         CFStringCompare(value, expected, 0) == kCFCompareEqualTo;
}

}  // namespace

bool PlatformBatteryState(BatteryState* state) {
  if (state == nullptr) return false;
  *state = BatteryState{};
  CFTypeRef snapshot = IOPSCopyPowerSourcesInfo();
  if (snapshot == nullptr) return false;
  CFStringRef providing = IOPSGetProvidingPowerSourceType(snapshot);
  state->external_power = providing != nullptr &&
      CFStringCompare(providing, CFSTR(kIOPMACPowerKey), 0) == kCFCompareEqualTo;
  CFArrayRef sources = IOPSCopyPowerSourcesList(snapshot);
  if (sources != nullptr) {
    for (CFIndex i = 0; i < CFArrayGetCount(sources); ++i) {
      CFDictionaryRef description =
          IOPSGetPowerSourceDescription(snapshot, CFArrayGetValueAtIndex(sources, i));
      if (description == nullptr ||
          !StringEquals(description, CFSTR(kIOPSTypeKey), CFSTR(kIOPSInternalBatteryType)) ||
          !BoolValue(description, CFSTR(kIOPSIsPresentKey))) {
        continue;
      }
      state->present = true;
      state->level = IntValue(description, CFSTR(kIOPSCurrentCapacityKey), 0);
      state->scale = IntValue(description, CFSTR(kIOPSMaxCapacityKey), 100);
      state->charging = BoolValue(description, CFSTR(kIOPSIsChargingKey));
      state->charged = BoolValue(description, CFSTR(kIOPSIsChargedKey));
      break;
    }
    CFRelease(sources);
  }
  CFRelease(snapshot);
  return true;
}

}  // namespace darwin_art::framework::power
