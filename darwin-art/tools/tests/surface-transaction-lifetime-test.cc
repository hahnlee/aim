#include "../../compat/window/surface_transaction_lifetime.h"

#include <cassert>
#include <cstdint>
#include <cstdio>
#include <csignal>
#include <string>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>
#include <cstdlib>
#include <new>

static bool fail_next_allocation = false;
void* operator new(std::size_t size) {
  if (fail_next_allocation) {
    fail_next_allocation = false;
    throw std::bad_alloc();
  }
  if (void* result = std::malloc(size == 0 ? 1 : size)) return result;
  throw std::bad_alloc();
}
void* operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void* pointer) noexcept { std::free(pointer); }
void operator delete[](void* pointer) noexcept { std::free(pointer); }
void operator delete(void* pointer, std::size_t) noexcept { std::free(pointer); }
void operator delete[](void* pointer, std::size_t) noexcept { std::free(pointer); }

using darwin_art::window::CompleteSurfaceTransaction;
using darwin_art::window::DiscardBufferCallbacks;
using darwin_art::window::DiscardTransactionCallbacks;
using darwin_art::window::ReleaseTransactionBuffers;
using darwin_art::window::ReleaseTransactionControls;
using darwin_art::window::SurfaceTransaction;
using darwin_art::window::SurfaceTransactionStats;

extern "C" void darwin_art_android_surface_transaction_clear(void* opaque);
extern "C" bool darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
    void*, void*, void*, void (*)(void*, ASurfaceTransactionStats*),
    void (*)(void*, int));
extern "C" void darwin_art_android_surface_transaction_set_buffer_callbacks(
    void*, void*, void*, void (*)(void*, ASurfaceTransactionStats*),
    void (*)(void*, int));
extern "C" int darwin_art_bionic_socket_broker_close(int fd);

namespace {

int buffer_releases = 0;
int control_releases = 0;
int fence_closes = 0;
int fence_dups = 0;
int fence_waits = 0;
int release_marks = 0;
int release_mark_pipe = -1;
std::vector<std::string> callback_order;
std::vector<int> discarded_fences;

template <typename T>
T* Fake(uintptr_t value) {
  return reinterpret_cast<T*>(value);
}

void CommitCallback(void*, ASurfaceTransactionStats* stats) {
  callback_order.emplace_back("commit");
  assert(ASurfaceTransactionStats_getPresentFenceFd(stats) == 301);
}

void CompleteCallback(void*, ASurfaceTransactionStats*) {
  callback_order.emplace_back("complete");
}

void BufferCompleteCallback(void*, ASurfaceTransactionStats*) {
  callback_order.emplace_back("buffer-complete");
}

void BufferDiscardCallback(void*, int fence) {
  callback_order.emplace_back("buffer-discard");
  discarded_fences.push_back(fence);
}

void ReentrantDiscardCallback(void* opaque) {
  callback_order.emplace_back("discard");
  // DiscardTransactionCallbacks clears its list before invoking callbacks.
  // Re-entry must therefore be a no-op rather than a second callback.
  DiscardTransactionCallbacks(static_cast<SurfaceTransaction*>(opaque));
}

void ClearAndReuseDuringComplete(void* opaque, ASurfaceTransactionStats*) {
  callback_order.emplace_back("reentrant-complete");
  auto* transaction = static_cast<SurfaceTransaction*>(opaque);
  darwin_art_android_surface_transaction_clear(transaction);
  auto& update = transaction->updates.emplace_back();
  update.opaque = Fake<ASurfaceControl>(0x1004);
  update.buffer = Fake<AHardwareBuffer>(0x2004);
  update.has_buffer = true;
}

void DeleteOriginalDuringComplete(void* opaque, ASurfaceTransactionStats*) {
  callback_order.emplace_back("delete-original");
  ASurfaceTransaction_delete(static_cast<ASurfaceTransaction*>(opaque));
}

struct DiscardReentry {
  SurfaceTransaction* transaction;
  bool delete_original;
};

void ClearOrDeleteDuringBufferDiscard(void* opaque, int fence) {
  discarded_fences.push_back(fence);
  if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
  auto* context = static_cast<DiscardReentry*>(opaque);
  if (context->delete_original) {
    ASurfaceTransaction_delete(
        reinterpret_cast<ASurfaceTransaction*>(context->transaction));
  } else {
    darwin_art_android_surface_transaction_clear(context->transaction);
    context->transaction->updates.push_back(
        {.buffer = Fake<AHardwareBuffer>(0x2008), .has_buffer = true});
  }
}

void ConsumePreparedDiscard(void*, int fence) {
  discarded_fences.push_back(fence);
  if (fence >= 0) (void)darwin_art_bionic_socket_broker_close(fence);
}

void DetachedOrdinaryDiscard(void*) {
  callback_order.emplace_back("detached-discard");
}

}  // namespace

extern "C" void AHardwareBuffer_release(AHardwareBuffer*) {
  ++buffer_releases;
}

extern "C" void ASurfaceControl_release(ASurfaceControl*) {
  ++control_releases;
}

extern "C" int darwin_art_bionic_socket_broker_close(int) {
  ++fence_closes;
  return 0;
}

extern "C" int darwin_art_bionic_socket_broker_dup(int fence) {
  ++fence_dups;
  if (fence == 8) return -1;
  return fence + 300;
}

extern "C" int sync_wait(int, int) {
  ++fence_waits;
  return 0;
}

extern "C" void darwin_art_android_mark_hardware_buffer_released(void*) {
  ++release_marks;
  if (release_mark_pipe >= 0) {
    const char mark = 'm';
    (void)write(release_mark_pipe, &mark, sizeof(mark));
  }
}

void ExpectAbortWithoutReleaseMark(bool previous, ASurfaceControl* control,
                                   AHardwareBuffer* buffer) {
  int pipe_fds[2];
  assert(pipe(pipe_fds) == 0);
  release_mark_pipe = pipe_fds[1];
  const pid_t child = fork();
  assert(child >= 0);
  if (child == 0) {
    close(pipe_fds[0]);
    SurfaceTransactionStats stats;
    stats.present_fence = 8;
    if (previous) {
      stats.controls.push_back(control);
      stats.previous_buffers.emplace(control, buffer);
      (void)ASurfaceTransactionStats_getPreviousReleaseFenceFd(
          reinterpret_cast<ASurfaceTransactionStats*>(&stats), control);
    } else {
      (void)ASurfaceTransactionStats_getPresentFenceFd(
          reinterpret_cast<ASurfaceTransactionStats*>(&stats));
    }
    _exit(0);
  }
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  close(pipe_fds[1]);
  release_mark_pipe = -1;
  char mark = 0;
  assert(read(pipe_fds[0], &mark, sizeof(mark)) == 0);
  close(pipe_fds[0]);
  assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
}

int main() {
  ASurfaceControl* control = Fake<ASurfaceControl>(0x1001);
  AHardwareBuffer* buffer = Fake<AHardwareBuffer>(0x2001);

  {
    SurfaceTransaction transaction;
    callback_order.clear();
    fail_next_allocation = true;
    assert(!darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
        &transaction, control, nullptr, BufferCompleteCallback,
        BufferDiscardCallback));
    assert(!fail_next_allocation && transaction.buffer_callbacks.empty());
    assert(callback_order.empty());
    assert(darwin_art::window::RegisterSurfaceTransactionBufferCallbacks(
        &transaction, control, &transaction, BufferCompleteCallback,
        BufferDiscardCallback));
    while (transaction.buffer_callbacks.size() <
           transaction.buffer_callbacks.capacity())
      transaction.buffer_callbacks.push_back(transaction.buffer_callbacks.front());
    const auto count = transaction.buffer_callbacks.size();
    fail_next_allocation = true;
    assert(!darwin_art::window::RegisterSurfaceTransactionBufferCallbacks(
        &transaction, control, nullptr, BufferCompleteCallback,
        BufferDiscardCallback));
    assert(!fail_next_allocation && transaction.buffer_callbacks.size() == count);
    assert(transaction.buffer_callbacks.front().context == &transaction);
    assert(callback_order.empty());
    transaction.buffer_callbacks.clear();
    assert(!darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
        nullptr, control, nullptr, BufferCompleteCallback, BufferDiscardCallback));
    assert(!darwin_art_android_surface_transaction_set_buffer_callbacks_checked(
        &transaction, control, nullptr, nullptr, BufferDiscardCallback));
    assert(transaction.buffer_callbacks.empty());
    const pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
      SurfaceTransaction fatal;
      fail_next_allocation = true;
      darwin_art_android_surface_transaction_set_buffer_callbacks(
          &fatal, control, nullptr, BufferCompleteCallback, BufferDiscardCallback);
      _exit(91);
    }
    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT);
  }

  // Discard removes ownership before invoking callbacks, forwards a dup of
  // the associated acquire fence, and is safe under callback re-entry.
  {
    SurfaceTransaction transaction;
    transaction.updates.push_back({.opaque = control,
                                   .buffer = buffer,
                                   .acquire_fence = 7,
                                   .has_buffer = true});
    transaction.buffer_callbacks.push_back(
        {control, nullptr, &BufferCompleteCallback, &BufferDiscardCallback});
    transaction.discards.push_back({&ReentrantDiscardCallback, &transaction});
    callback_order.clear();
    discarded_fences.clear();
    DiscardTransactionCallbacks(&transaction);
    assert((callback_order == std::vector<std::string>{"buffer-discard",
                                                       "discard"}));
    assert(discarded_fences == std::vector<int>{307});
    assert(transaction.buffer_callbacks.empty() && transaction.discards.empty());
    assert(fence_dups == 1);
    ReleaseTransactionBuffers(&transaction);
    assert(buffer_releases == 1 && fence_closes == 1);
  }

  // Dup failure immediately quarantines the producer slot and forwards -2;
  // shutdown must not synchronously wait on an unsignaled producer fence.
  {
    SurfaceTransaction transaction;
    transaction.updates.push_back({.opaque = control, .acquire_fence = 8});
    transaction.buffer_callbacks.push_back(
        {control, nullptr, &BufferCompleteCallback, &BufferDiscardCallback});
    discarded_fences.clear();
    const int waits_before = fence_waits;
    DiscardBufferCallbacks(&transaction, control);
    assert(discarded_fences == std::vector<int>{-2});
    assert(fence_waits == waits_before);
    transaction.buffer_callbacks.push_back(
        {control, nullptr, &BufferCompleteCallback, &BufferDiscardCallback});
    DiscardBufferCallbacks(&transaction, control, true);
    assert((discarded_fences == std::vector<int>{-2, -2}));
  }

  // Completion order is commit, complete, then buffer release; all callback
  // lists are detached before dispatch and remaining ownership is released.
  {
    SurfaceTransaction transaction;
    transaction.controls.push_back(control);
    transaction.updates.push_back({.opaque = control,
                                   .buffer = buffer,
                                   .acquire_fence = -1,
                                   .has_buffer = true});
    transaction.commits.push_back({&CommitCallback, nullptr});
    transaction.completes.push_back({&CompleteCallback, nullptr});
    transaction.buffer_callbacks.push_back(
        {control, nullptr, &BufferCompleteCallback, &BufferDiscardCallback});
    SurfaceTransactionStats stats;
    stats.present_fence = 1;
    callback_order.clear();
    CompleteSurfaceTransaction(&transaction, &stats);
    assert((callback_order == std::vector<std::string>{"commit", "complete",
                                                       "buffer-complete"}));
    assert(transaction.controls.empty() && transaction.updates.empty());
    assert(buffer_releases == 2 && control_releases == 1);
  }

  // A complete callback may clear and reuse the original transaction. The
  // detached batch must release only its old payload, leaving the new payload
  // owned by the reused transaction.
  {
    SurfaceTransaction transaction;
    transaction.updates.push_back({.opaque = control,
                                   .buffer = buffer,
                                   .acquire_fence = -1,
                                   .has_buffer = true});
    transaction.commits.push_back(
        {&ClearAndReuseDuringComplete, &transaction});
    SurfaceTransactionStats stats;
    callback_order.clear();
    CompleteSurfaceTransaction(&transaction, &stats);
    assert((callback_order ==
            std::vector<std::string>{"reentrant-complete"}));
    assert(transaction.updates.size() == 1);
    assert(transaction.updates[0].buffer == Fake<AHardwareBuffer>(0x2004));
    assert(buffer_releases == 3);
    ReleaseTransactionBuffers(&transaction);
    assert(buffer_releases == 4);
  }

  // The original opaque object is already empty when a callback deletes it;
  // CompleteSurfaceTransaction performs no access through that pointer after
  // dispatch and still releases the detached payload exactly once.
  {
    auto* opaque = ASurfaceTransaction_create();
    assert(opaque != nullptr);
    auto* transaction = reinterpret_cast<SurfaceTransaction*>(opaque);
    transaction->updates.push_back({.opaque = control,
                                    .buffer = buffer,
                                    .acquire_fence = -1,
                                    .has_buffer = true});
    transaction->commits.push_back({&DeleteOriginalDuringComplete, opaque});
    SurfaceTransactionStats stats;
    callback_order.clear();
    CompleteSurfaceTransaction(transaction, &stats);
    assert((callback_order == std::vector<std::string>{"delete-original"}));
    assert(buffer_releases == 5);
  }

  // Stats owns previous-buffer references and its present fence. The previous
  // fence query marks the exact buffer and duplicates the present fence.
  {
    SurfaceTransactionStats stats;
    stats.controls.push_back(control);
    stats.previous_buffers.emplace(control, buffer);
    stats.present_fence = 1;
    assert(ASurfaceTransactionStats_getPreviousReleaseFenceFd(
               reinterpret_cast<ASurfaceTransactionStats*>(&stats), control) ==
           301);
    assert(release_marks == 1 && fence_dups == 4);
  }
  assert(buffer_releases == 6 && fence_closes == 3);

  // An absent previous buffer or present fence is the ordinary NDK -1 path;
  // it does not duplicate a descriptor or mark any buffer as released.
  {
    SurfaceTransactionStats stats;
    stats.controls.push_back(control);
    const int dups_before = fence_dups;
    const int marks_before = release_marks;
    assert(ASurfaceTransactionStats_getPreviousReleaseFenceFd(
               reinterpret_cast<ASurfaceTransactionStats*>(&stats), control) ==
           -1);
    assert(ASurfaceTransactionStats_getPresentFenceFd(
               reinterpret_cast<ASurfaceTransactionStats*>(&stats)) == -1);
    assert(fence_dups == dups_before && release_marks == marks_before);
  }

  // A real release-fence descriptor cannot be represented by NDK -1 when dup
  // fails.  The public getter terminates, and the child-side pipe proves that
  // previous-buffer release marking did not happen before the fatal failure.
  ExpectAbortWithoutReleaseMark(true, control, buffer);
  ExpectAbortWithoutReleaseMark(false, control, buffer);

  // Prepare BOTH transferred fences before the first buffer callback clears
  // or deletes the original. Ordinary discard must likewise be detached, and
  // callback-added source state must survive the outer operation.
  for (bool delete_original : {false, true}) {
    auto* transaction = new SurfaceTransaction;
    const auto second_control = Fake<ASurfaceControl>(0x1009);
    transaction->updates.push_back({.opaque = control, .buffer = buffer,
                                    .acquire_fence = 7, .has_buffer = true});
    transaction->updates.push_back({.opaque = second_control, .buffer = buffer,
                                    .acquire_fence = 9, .has_buffer = true});
    DiscardReentry context{transaction, delete_original};
    transaction->buffer_callbacks.push_back(
        {control, &context, nullptr, &ClearOrDeleteDuringBufferDiscard});
    transaction->buffer_callbacks.push_back(
        {second_control, nullptr, nullptr, &ConsumePreparedDiscard});
    transaction->discards.push_back({&DetachedOrdinaryDiscard, nullptr});
    const int closes_before = fence_closes;
    const int releases_before = buffer_releases;
    const int dups_before = fence_dups;
    discarded_fences.clear();
    callback_order.clear();
    DiscardTransactionCallbacks(transaction);
    assert((discarded_fences == std::vector<int>{307, 309}));
    assert((callback_order == std::vector<std::string>{"detached-discard"}));
    assert(fence_dups == dups_before + 2);
    assert(fence_closes == closes_before + 4);
    assert(buffer_releases == releases_before + 2);
    if (!delete_original) {
      assert(transaction->updates.size() == 1);
      assert(transaction->updates[0].buffer == Fake<AHardwareBuffer>(0x2008));
      ASurfaceTransaction_delete(reinterpret_cast<ASurfaceTransaction*>(transaction));
      assert(buffer_releases == releases_before + 3);
    }
  }

  std::puts("SurfaceTransaction lifetime: discard fence forwarding, reentrant "
            "cleanup including buffer-discard clear/delete/reuse, completion "
            "order, stats ownership, and release-once PASS");
  return 0;
}
