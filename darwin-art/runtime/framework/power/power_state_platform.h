#pragma once

namespace darwin_art::framework::power {

bool PlatformIsInteractive();

// Host power source reduced to the fields Android's health HAL reports.
struct BatteryState {
  bool present = false;       // an internal battery exists
  int level = 0;              // current capacity
  int scale = 100;            // capacity at full charge
  bool charging = false;
  bool charged = false;
  bool external_power = false;  // drawing from AC
};

// Reads IOPowerSources; false when the host power-source list is unavailable.
bool PlatformBatteryState(BatteryState* state);

}  // namespace darwin_art::framework::power
