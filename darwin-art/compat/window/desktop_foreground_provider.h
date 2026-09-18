#pragma once

#include <cstdint>

namespace darwin_art::window {

// A process identity includes its Darwin birth time so a recycled PID cannot
// inherit foreground authority from its predecessor.
struct ProcessIdentity final {
  int pid = 0;
  std::uint64_t start_seconds = 0;
  std::uint64_t start_microseconds = 0;
};

// Captures the exact BSD process identity for pid.  The output is written
// only after proc_pidinfo has returned a complete, self-consistent record.
bool CaptureProcessIdentity(int pid, ProcessIdentity* output) noexcept;

// Performs a fresh front-process query and revalidates the target's birth
// time on both sides of that query.  This is a host fact only: it has no
// window lookup, transition cache, serial, or Android focus policy.
bool IsProcessForeground(const ProcessIdentity& identity) noexcept;

}  // namespace darwin_art::window
