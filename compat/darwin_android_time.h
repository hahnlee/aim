#pragma once

#include <mach/mach_time.h>
#include <time.h>

#include <cstdint>

namespace aim {

inline std::int64_t MachTicksToNanos(std::uint64_t ticks) {
  static const mach_timebase_info_data_t timebase = [] {
    mach_timebase_info_data_t value{};
    mach_timebase_info(&value);
    return value;
  }();
  return static_cast<std::int64_t>(
      (static_cast<unsigned __int128>(ticks) * timebase.numer) /
      timebase.denom);
}

// Android requires System.nanoTime(), SystemClock.uptime*(), input event times,
// and Choreographer frame timestamps to share one monotonic domain. ART's
// java.lang.System.nanoTime intrinsic resolves to CLOCK_MONOTONIC on Darwin,
// so every compatibility provider must use that same clock. Mixing it with
// mach_absolute_time() creates a sleep-time offset and makes Choreographer
// report millions of skipped frames after the host has slept.
inline std::int64_t AndroidUptimeNanos() {
  timespec value{};
  return clock_gettime(CLOCK_MONOTONIC, &value) == 0
             ? static_cast<std::int64_t>(value.tv_sec) * 1'000'000'000LL +
                   value.tv_nsec
             : 0;
}

inline std::int64_t AndroidElapsedRealtimeNanos() {
  return MachTicksToNanos(mach_continuous_time());
}

}  // namespace aim
