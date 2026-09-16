#include "power_state_platform.h"

#include <CoreGraphics/CGDirectDisplay.h>
#include <CoreGraphics/CGDisplayConfiguration.h>

namespace darwin_art::framework::power {

bool PlatformIsInteractive() {
  const CGDirectDisplayID display = CGMainDisplayID();
  return display != kCGNullDirectDisplay && !CGDisplayIsAsleep(display);
}

}  // namespace darwin_art::framework::power
