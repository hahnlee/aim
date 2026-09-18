#include "compat/surfaceflinger/transaction_bridge.h"
#include "compat/window/surface_control_registry.h"
#include "compat/window/surface_control_submit_darwin.h"
#include "compat/window/surface_transaction_lifetime.h"
#include "compat/window/surface_transaction_submission.h"
#include "compat/window/surface_transaction_state.h"

#include <android/hardware_buffer.h>

#include <cassert>
#include <cerrno>
#include <csignal>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

struct AHardwareBuffer {};

namespace {
// This is a common-owner fixture: the registry/state/ready-owner TUs are real,
// while the typed AHB, Darwin query/submit and AOSP frontend seams are local
// callbacks. It intentionally does not claim full SurfaceFlinger IPC coverage.
AHardwareBuffer* const kBuffer =
    reinterpret_cast<AHardwareBuffer*>(static_cast<uintptr_t>(0x101));
int g_acquires = 0;
int g_releases = 0;
int g_frontend_calls = 0;
int g_submit_calls = 0;
bool g_frontend_accepts = true;
bool g_submit_succeeds = true;
int g_present_fence = 71;
size_t g_submit_presentations = 0;
uint32_t g_disposition = DARWIN_ART_SF_COMMIT_COMMITTED;
bool g_context_restored = true;
ASurfaceControl* g_expected_unlatched = nullptr;
darwin_art::window::SurfaceTransaction* g_pending = nullptr;

void SetEnvironment(bool central) {
  unsetenv("DARWIN_ART_APK_APP_PACKAGE");
  if (central) setenv("DARWIN_ART_SURFACEFLINGER_SOCKET", "central", 1);
  else unsetenv("DARWIN_ART_SURFACEFLINGER_SOCKET");
}

darwin_art::window::SurfaceTransaction MakeTransaction(ASurfaceControl* control) {
  darwin_art::window::SurfaceTransaction transaction;
  transaction.controls.push_back(control);
  transaction.updates.push_back({
      .opaque = control, .buffer = kBuffer, .has_buffer = true});
  return transaction;
}

AHardwareBuffer* FindBuffer(ASurfaceControl* control,
                            darwin_art::window::SurfaceTransaction* transaction) {
  std::vector<darwin_art::window::SurfaceControlStateView> controls;
  std::vector<darwin_art::window::SurfaceControlUpdateView> updates;
  assert(darwin_art::window::SurfaceControlRegistry::Instance().CopyViews(
      transaction, &controls, &updates));
  for (const auto& view : controls) {
    if (view.opaque == control) return view.buffer;
  }
  return nullptr;
}
}

extern "C" void AHardwareBuffer_acquire(AHardwareBuffer* buffer) {
  assert(buffer != nullptr);
  ++g_acquires;
}
extern "C" void AHardwareBuffer_release(AHardwareBuffer* buffer) {
  assert(buffer != nullptr);
  ++g_releases;
}
extern "C" void AHardwareBuffer_describe(
    const AHardwareBuffer* buffer, AHardwareBuffer_Desc* description) {
  assert(buffer != nullptr && description != nullptr);
  *description = {};
  description->width = 32;
  description->height = 16;
  description->layers = 1;
}
extern "C" int darwin_art_bionic_socket_broker_close(int fd) {
  assert(fd >= 0);
  return 0;
}

namespace darwin_art::window {
SurfaceTransactionStats::~SurfaceTransactionStats() {
  for (const auto& entry : previous_buffers) AHardwareBuffer_release(entry.second);
  if (present_fence >= 0) (void)darwin_art_bionic_socket_broker_close(present_fence);
}

SurfaceControlSubmissionEnvironment QuerySurfaceControlSubmissionEnvironment() {
  return {.central_service = getenv("DARWIN_ART_SURFACEFLINGER_SOCKET") != nullptr,
          .application = getenv("DARWIN_ART_APK_APP_PACKAGE") != nullptr};
}

SurfaceControlSubmitResult SubmitSurfaceControlDarwin(
    const SurfaceControlSnapshot& snapshot, uint64_t, bool) {
  ++g_submit_calls;
  g_submit_presentations = snapshot.presentations.size();
  if (g_expected_unlatched != nullptr) {
    assert(FindBuffer(g_expected_unlatched, g_pending) == nullptr);
    assert(g_pending->updates[0].buffer == kBuffer);
    assert(snapshot.presentations.size() == 1);
    assert(snapshot.presentations[0].buffer == kBuffer);
  }
  return {.success = g_submit_succeeds, .present_fence = g_present_fence,
          .receipt = {g_disposition, g_submit_succeeds ? 0 : EIO,
                      g_present_fence},
          .context_restored = g_context_restored};
}
}

extern "C" bool darwin_art_surfaceflinger_commit_transaction(
    uint64_t transaction_id, const DarwinArtSurfaceFlingerLayerUpdate* updates,
    size_t update_count, DarwinArtSurfaceFlingerCommitResult* result) {
  assert(transaction_id != 0 && updates != nullptr && update_count == 1);
  ++g_frontend_calls;
  if (result != nullptr) {
    result->transaction_id = transaction_id;
    result->transaction_count = 1;
    result->layer_state_count = update_count;
  }
  return g_frontend_accepts;
}

static void TestFrontendRejectionPreservesOwnership() {
  SetEnvironment(false);
  g_frontend_accepts = false;
  auto& registry = darwin_art::window::SurfaceControlRegistry::Instance();
  auto* control = registry.Create(nullptr, "rejected", true);
  auto transaction = MakeTransaction(control);
  darwin_art::window::SurfaceTransactionStats stats;
  assert(!darwin_art::window::ApplyReadySurfaceTransaction(&transaction, &stats));
  assert(g_frontend_calls == 1 && g_submit_calls == 0);
  assert(transaction.updates[0].buffer == kBuffer);
  assert(FindBuffer(control, &transaction) == nullptr);
  assert(g_acquires == g_releases);
  registry.Release(control);
}

static void TestAcceptedTransferAndSnapshot() {
  SetEnvironment(false);
  g_frontend_accepts = true;
  g_submit_succeeds = true;
  g_present_fence = 71;
  auto& registry = darwin_art::window::SurfaceControlRegistry::Instance();
  auto* control = registry.Create(nullptr, "accepted", true);
  auto transaction = MakeTransaction(control);
  g_expected_unlatched = control;
  g_pending = &transaction;
  const int balance = g_acquires - g_releases;
  {
    darwin_art::window::SurfaceTransactionStats stats;
    assert(darwin_art::window::ApplyReadySurfaceTransaction(&transaction, &stats));
    assert(transaction.updates[0].buffer == nullptr);
    assert(FindBuffer(control, &transaction) == kBuffer);
    assert(g_submit_calls == 1 && g_submit_presentations == 1);
    assert(stats.present_fence == g_present_fence);
  }
  g_expected_unlatched = nullptr;
  g_pending = nullptr;
  assert(g_acquires - g_releases == balance + 1);
  registry.Release(control);
  assert(g_acquires - g_releases == balance - 1);
}

static void TestCentralRejectionPreservesState() {
  SetEnvironment(true);
  g_submit_succeeds = false;
  g_disposition = DARWIN_ART_SF_COMMIT_REJECTED;
  g_present_fence = -1;
  auto& registry = darwin_art::window::SurfaceControlRegistry::Instance();
  auto* control = registry.Create(nullptr, "central-rejected", true);
  auto transaction = MakeTransaction(control);
  const int balance = g_acquires - g_releases;
  darwin_art::window::SurfaceTransactionStats stats;
  assert(!darwin_art::window::ApplyReadySurfaceTransaction(&transaction, &stats));
  assert(FindBuffer(control, &transaction) == nullptr);
  assert(transaction.updates[0].buffer == kBuffer);
  assert(stats.controls.empty() && stats.present_fence == -1);
  assert(g_acquires - g_releases == balance);
  registry.Release(control);
  g_disposition = DARWIN_ART_SF_COMMIT_UNKNOWN;
}

static void TestCentralSubmitFailureAbortsAfterCommit() {
  SetEnvironment(true);
  g_submit_succeeds = false;
  auto& registry = darwin_art::window::SurfaceControlRegistry::Instance();
  auto* control = registry.Create(nullptr, "fatal", true);
  auto transaction = MakeTransaction(control);
  const pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    darwin_art::window::SurfaceTransactionStats stats;
    (void)darwin_art::window::ApplyReadySurfaceTransaction(&transaction, &stats);
    _exit(0);
  }
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
  registry.Release(control);
  g_submit_succeeds = true;
}

int main() {
  TestFrontendRejectionPreservesOwnership();
  TestAcceptedTransferAndSnapshot();
  TestCentralRejectionPreservesState();
  TestCentralSubmitFailureAbortsAfterCommit();
  std::puts("surface-control ready transaction: rejection ownership, accepted transfer/snapshot, post-commit fatal PASS");
}
