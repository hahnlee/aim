#include "compat/window/desktop_foreground_provider.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <unistd.h>

using darwin_art::window::CaptureProcessIdentity;
using darwin_art::window::IsProcessForeground;
using darwin_art::window::ProcessIdentity;

int main() {
  ProcessIdentity self;
  assert(CaptureProcessIdentity(getpid(), &self));
  assert(self.pid == getpid());
  assert(self.start_seconds != 0);
  assert(self.start_microseconds < 1000000);

  assert(!CaptureProcessIdentity(0, &self));
  assert(!CaptureProcessIdentity(-1, &self));
  assert(!CaptureProcessIdentity(getpid(), nullptr));

  ProcessIdentity forged = self;
  ++forged.start_seconds;
  assert(!IsProcessForeground(forged));

  // Foreground status is an intentionally live host fact; either answer is
  // valid in a headless/test runner, but the query must complete safely.
  const bool foreground = IsProcessForeground(self);
  std::printf("desktop foreground provider: self pid=%d foreground=%s PASS\n",
              self.pid, foreground ? "true" : "false");
  return 0;
}
