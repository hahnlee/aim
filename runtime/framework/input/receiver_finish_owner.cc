#include "receiver_finish_owner.h"

#include "input_routing.h"
#include "input_transport.h"
#include <algorithm>
#include <chrono>
#include <mutex>
#include <stdexcept>
#include <thread>
#include <utility>
#include <vector>

namespace aim::input {
namespace {
bool CanReserve(InputEventOrigin origin,
                const std::shared_ptr<const InputRoutingRecipient>& original) {
  if (original == nullptr) return false;
  switch (origin.kind) {
    case ReceiverPacketOrigin::kLocalQueue:
      return true;
    case ReceiverPacketOrigin::kImportedChannel:
      return original->originalendpoint != nullptr &&
             original->originalendpoint->transport != nullptr &&
             original->originalendpoint->transport->RemoteEndpointFd() >= 0;
  }
  return false;
}

// Live receivers and the watchdog state. Receivers unregister under the same
// mutex the watchdog inspects them under, so an inspected owner is alive.
struct AnrMonitor {
  std::mutex mutex;
  std::vector<const ReceiverFinishOwner*> owners;
  // (owner, sequence) pairs already reported: one report per stuck event.
  std::vector<std::pair<const ReceiverFinishOwner*, uint32_t>> reported;
  InputAnrReporter reporter = nullptr;
  uint64_t timeout_ns = 0;
  bool watchdog_started = false;
};

AnrMonitor& Monitor() {
  static auto* monitor = new AnrMonitor();
  return *monitor;
}

uint64_t SteadyNowNs() {
  return static_cast<uint64_t>(
      std::chrono::duration_cast<std::chrono::nanoseconds>(
          std::chrono::steady_clock::now().time_since_epoch())
          .count());
}
}  // namespace

ReceiverFinishOwner::ReceiverFinishOwner() {
  AnrMonitor& monitor = Monitor();
  std::lock_guard<std::mutex> lock(monitor.mutex);
  monitor.owners.push_back(this);
}

ReceiverFinishOwner::~ReceiverFinishOwner() {
  AnrMonitor& monitor = Monitor();
  std::lock_guard<std::mutex> lock(monitor.mutex);
  monitor.owners.erase(
      std::remove(monitor.owners.begin(), monitor.owners.end(), this),
      monitor.owners.end());
  monitor.reported.erase(
      std::remove_if(monitor.reported.begin(), monitor.reported.end(),
                     [this](const auto& entry) { return entry.first == this; }),
      monitor.reported.end());
}

bool ReceiverFinishOwner::OldestUnfinished(uint32_t* sequence,
                                           uint64_t* registered_ns) const {
  return ledger_.OldestUnfinished(sequence, registered_ns);
}

size_t CheckInputAnrs(uint64_t now_ns) {
  AnrMonitor& monitor = Monitor();
  std::vector<std::pair<uint32_t, uint64_t>> reports;
  InputAnrReporter reporter = nullptr;
  {
    std::lock_guard<std::mutex> lock(monitor.mutex);
    reporter = monitor.reporter;
    if (reporter == nullptr) return 0;
    // Forget reports whose event has since finished.
    monitor.reported.erase(
        std::remove_if(monitor.reported.begin(), monitor.reported.end(),
                       [](const auto& entry) {
                         uint32_t sequence = 0;
                         uint64_t since = 0;
                         return !entry.first->OldestUnfinished(&sequence, &since) ||
                                sequence != entry.second;
                       }),
        monitor.reported.end());
    for (const ReceiverFinishOwner* owner : monitor.owners) {
      uint32_t sequence = 0;
      uint64_t since = 0;
      if (!owner->OldestUnfinished(&sequence, &since) || now_ns < since ||
          now_ns - since < monitor.timeout_ns) {
        continue;
      }
      const auto key = std::make_pair(owner, sequence);
      if (std::find(monitor.reported.begin(), monitor.reported.end(), key) !=
          monitor.reported.end()) {
        continue;
      }
      monitor.reported.push_back(key);
      reports.emplace_back(sequence, (now_ns - since) / 1'000'000);
    }
  }
  // Report outside the lock: the reporter makes a Binder call.
  for (const auto& [sequence, waited_ms] : reports) reporter(sequence, waited_ms);
  return reports.size();
}

void InstallInputAnrReporter(InputAnrReporter report, uint64_t timeout_ns) {
  AnrMonitor& monitor = Monitor();
  std::lock_guard<std::mutex> lock(monitor.mutex);
  monitor.reporter = report;
  monitor.timeout_ns = timeout_ns;
  if (report == nullptr || monitor.watchdog_started) return;
  monitor.watchdog_started = true;
  std::thread([] {
    for (;;) {
      std::this_thread::sleep_for(std::chrono::milliseconds(500));
      CheckInputAnrs(SteadyNowNs());
    }
  }).detach();
}

bool ReceiverFinishOwner::Reserve(
    uint32_t sequence, InputEventOrigin origin,
    std::shared_ptr<const InputRoutingRecipient> original) {
  if (!CanReserve(origin, original)) return false;
  return ledger_.TryRegister(sequence, origin, std::move(original));
}

bool ReceiverFinishOwner::ReserveNext(
    uint32_t* next_sequence, uint32_t* allocated, InputEventOrigin origin,
    std::shared_ptr<const InputRoutingRecipient> original) {
  if (!CanReserve(origin, original)) return false;
  return ledger_.TryRegisterNext(next_sequence, allocated, origin,
                                 std::move(original));
}

RemoteFinishState ReceiverFinishOwner::SealRemoteAdmission() {
  return ledger_.SealRemoteAdmission();
}

RemoteFinishState ReceiverFinishOwner::RemoteState() const {
  return ledger_.RemoteState();
}

bool ReceiverFinishOwner::HasOutstandingRemote() const {
  return ledger_.HasOutstandingRemote();
}

bool ReceiverFinishOwner::Cancel(uint32_t sequence) {
  return ledger_.Cancel(sequence);
}

bool ReceiverFinishOwner::CloseObservation(uint32_t sequence, bool* handled) {
  return ledger_.CloseObservation(sequence, handled);
}

ReceiverFinishProgress ReceiverFinishOwner::Finish(uint32_t sequence, bool handled) {
  if (!ledger_.Record(sequence, handled)) return {};
  return RetryPending();
}

bool ReceiverFinishOwner::HasPendingAck() const {
  return ledger_.HasPendingAck();
}

ReceiverFinishProgress ReceiverFinishOwner::RetryPending(size_t budget) {
  ReceiverFinishProgress progress;
  if (retrying_.test_and_set(std::memory_order_acquire)) {
    progress.coalesced = progress.retry_needed = true;
    return progress;
  }
  struct Admission {
    std::atomic_flag& flag;
    bool active = true;
    void Release() { flag.clear(std::memory_order_release); active = false; }
    ~Admission() { if (active) Release(); }
  } admission{retrying_};
  for (size_t count = 0; count < budget; ++count) {
    FinishAck ack;
    uint64_t ticket = 0;
    if (!ledger_.ClaimPendingAck(&ack, &ticket)) break;
    struct Claim {
      FinishLedger& ledger;
      uint64_t ticket;
      bool settled = false;
      ~Claim() { if (!settled) (void)ledger.CompleteAckSubmission(ticket, false); }
    } claim{ledger_, ticket};
    InputTransportStatus status = InputTransportStatus::kAccepted;
    std::shared_ptr<InputTransport> transport;
    if (ack.origin.kind == ReceiverPacketOrigin::kImportedChannel) {
      transport = ack.recipient->originalendpoint->transport;
      status = transport->IsTxTerminal() ? InputTransportStatus::kTerminal
          : SendInputTransportAck64(transport.get(), ack.origin.sequence, ack.handled);
    }
    if (status == InputTransportStatus::kBackpressured) {
      progress.backpressured = true;
      // Empty-TX allocation/counter rejection has no writable wake source.
      // Retain it for explicit owner recovery, never spin an empty FD.
      progress.recovery_needed = !transport->HasPendingTx();
      break;
    }
    const bool terminal = status == InputTransportStatus::kTerminal;
    claim.settled = terminal ? ledger_.CompleteAckTerminal(ticket)
                            : ledger_.CompleteAckSubmission(ticket, true);
    if (!claim.settled) throw std::logic_error("Lost exact receiver ACK claim");
    if (terminal) ++progress.terminal;
    else ++progress.accepted;
  }
  // Release admission BEFORE the final pending scan. A concurrent/reentrant
  // finish either appears in this scan, or acquires admission itself; there
  // is no final-empty-scan/busy-clear lost-wakeup interval.
  admission.Release();
  progress.retry_needed = ledger_.HasPendingAck();
  return progress;
}

}  // namespace aim::input
