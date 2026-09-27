// The input dispatching watchdog: an event handed to a receiver and not
// finished within the timeout is reported exactly once; a finished event
// clears its report; unregistered receivers are forgotten.
#include "runtime/framework/input/input_routing.h"
#include "runtime/framework/input/input_transport.h"
#include "runtime/framework/input/receiver_finish_owner.h"

#include <cassert>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <vector>

// Local-queue receivers never reach a remote transport; fail if they do.
namespace darwin_art::input {
InputTransportStatus SendInputTransportAck64(InputTransport*, uint64_t, bool) {
  std::abort();
}
bool InputTransport::HasPendingTx() const { std::abort(); }
bool InputTransport::IsTxTerminal() const { std::abort(); }
int InputTransport::RemoteEndpointFd() const { std::abort(); }
}  // namespace darwin_art::input

namespace {
using darwin_art::input::CheckInputAnrs;
using darwin_art::input::InputEventOrigin;
using darwin_art::input::InputRoutingRecipient;
using darwin_art::input::ReceiverFinishOwner;

std::vector<std::pair<uint32_t, uint64_t>> g_reports;

void Record(uint32_t sequence, uint64_t waited_ms) {
  g_reports.emplace_back(sequence, waited_ms);
}

uint64_t Now() {
  return static_cast<uint64_t>(
      std::chrono::duration_cast<std::chrono::nanoseconds>(
          std::chrono::steady_clock::now().time_since_epoch())
          .count());
}

// A local-queue reservation needs a recipient but never dereferences it.
std::shared_ptr<const InputRoutingRecipient> Recipient() {
  static char storage;
  return std::shared_ptr<const InputRoutingRecipient>(
      reinterpret_cast<const InputRoutingRecipient*>(&storage),
      [](const InputRoutingRecipient*) {});
}
}  // namespace

int main() {
  constexpr uint64_t kSecond = 1'000'000'000ull;
  // Without a reporter nothing is watched.
  assert(CheckInputAnrs(Now() + 60 * kSecond) == 0);
  darwin_art::input::InstallInputAnrReporter(&Record, 5 * kSecond);

  auto owner = std::make_unique<ReceiverFinishOwner>();
  assert(owner->Reserve(11, InputEventOrigin{}, Recipient()));
  const uint64_t start = Now();
  assert(CheckInputAnrs(start + 4 * kSecond) == 0);
  assert(CheckInputAnrs(start + 6 * kSecond) == 1);
  assert(g_reports.size() == 1 && g_reports[0].first == 11 &&
         g_reports[0].second >= 5000);
  // Reported once per stuck event.
  assert(CheckInputAnrs(start + 20 * kSecond) == 0);

  // Once finished, a later stuck event is reported again.
  owner->Finish(11, true);
  assert(owner->Reserve(12, InputEventOrigin{}, Recipient()));
  assert(CheckInputAnrs(Now() + 6 * kSecond) == 1);
  assert(g_reports.back().first == 12);

  // A destroyed receiver is no longer inspected.
  owner.reset();
  assert(CheckInputAnrs(Now() + 60 * kSecond) == 0);
  std::puts("input ANR monitor: timeout/once/re-arm/unregister PASS");
}
