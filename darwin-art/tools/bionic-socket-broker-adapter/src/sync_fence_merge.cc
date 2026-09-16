#include "sync_fence_merge.h"

#include <dispatch/dispatch.h>
#include <fcntl.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include <array>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::socket {
namespace {

dispatch_queue_t SharedMergeQueue() {
  static dispatch_once_t once = 0;
  static dispatch_queue_t queue = nullptr;
  dispatch_once_f(&once, nullptr, [](void*) {
    queue = dispatch_queue_create("darwin-art.sync-fence-merge",
                                  DISPATCH_QUEUE_SERIAL);
  });
  return queue;
}

void CloseIfOwned(int* fd) noexcept {
  if (fd == nullptr || *fd < 0) return;
  (void)close(*fd);
  *fd = -1;
}

void CloseDistinct(int first, int second, int output) noexcept {
  if (first >= 0) (void)close(first);
  if (second >= 0 && second != first) (void)close(second);
  if (output >= 0 && output != first && output != second)
    (void)close(output);
}

void CloseUnconfiguredSource(void* opaque) {
  const uintptr_t encoded = reinterpret_cast<uintptr_t>(opaque);
  if (encoded == 0) return;
  const int fd = static_cast<int>(encoded - 1);
  if (fd >= 0) (void)close(fd);
}

void CancelUnconfiguredSource(dispatch_source_t source, int* owned_fd) noexcept {
  if (source == nullptr || owned_fd == nullptr) return;
  const int fd = std::exchange(*owned_fd, -1);
  if (fd < 0) return;
  // A newly-created source is suspended.  Releasing it while suspended is
  // invalid on Darwin.  Keep the monitored FD owned by this cancellation
  // handler until dispatch has detached the source; AbortCreate must not
  // close it early and permit numeric FD reuse.
  dispatch_set_context(source,
                       reinterpret_cast<void*>(static_cast<uintptr_t>(fd) +
                                               1));
  dispatch_source_set_cancel_handler_f(source, &CloseUnconfiguredSource);
  dispatch_resume(source);
  dispatch_source_cancel(source);
  dispatch_release(source);
}

bool SetCloseOnExec(int fd) noexcept {
  const int flags = fcntl(fd, F_GETFD);
  return flags >= 0 && fcntl(fd, F_SETFD, flags | FD_CLOEXEC) == 0;
}

bool SetNoSigPipe(int fd) noexcept {
#if defined(F_SETNOSIGPIPE)
  return fcntl(fd, F_SETNOSIGPIPE, 1) == 0;
#else
  (void)fd;
  return false;
#endif
}

}  // namespace

struct SyncFenceMerge::Impl final {
  struct SourceContext {
    std::shared_ptr<Impl> impl;
    size_t index = 0;
  };

  Impl(int first, int second, int output, std::shared_ptr<void> retained)
      : input_fds{first, second}, output_fd(output),
        retained_inputs(std::move(retained)) {}

  ~Impl() { Cancel(); }

  void Cancel() noexcept;
  void MarkReady(size_t index, bool input_failed = false) noexcept;
  void AbortCreate() noexcept;
  static void CancelSources(
      std::array<dispatch_source_t, 2> sources) noexcept;

  static void SourceEventHandler(void* opaque);
  static void SourceCancelHandler(void* opaque);

  std::mutex mutex;
  // Concurrent close/completion must not return from Cancel while another
  // cancellation still holds the upstream broker descriptions being released.
  std::mutex cancellation_mutex;
  std::array<int, 2> input_fds;
  int output_fd;
  std::array<bool, 2> ready{false, false};
  bool cancelled = false;
  bool signalled = false;
  bool failed = false;
  std::shared_ptr<void> retained_inputs;
  std::array<dispatch_source_t, 2> sources{nullptr, nullptr};
};

void SyncFenceMerge::Impl::SourceEventHandler(void* opaque) {
  auto* context = static_cast<SyncFenceMerge::Impl::SourceContext*>(opaque);
  if (context == nullptr || context->impl == nullptr) return;
  // READ sources also fire on EOF. Our pipe-backed fence protocol requires
  // a full marker; loss of the producer without a marker is an error, not a
  // completed GPU operation. Do not consume the guest-visible marker.
  int input = -1;
  {
    std::lock_guard lock(context->impl->mutex);
    if (context->impl->cancelled || context->impl->signalled) return;
    input = context->impl->input_fds[context->index];
  }
  int available = 0;
  if (ioctl(input, FIONREAD, &available) != 0 ||
      available != static_cast<int>(sizeof(uint64_t))) {
    // Error is terminal for this input, not permission to release the other
    // GPU dependency early. Publish output EOF only after BOTH are terminal.
    context->impl->MarkReady(context->index, true);
    return;
  }
  context->impl->MarkReady(context->index);
}

void SyncFenceMerge::Impl::SourceCancelHandler(void* opaque) {
  std::unique_ptr<SyncFenceMerge::Impl::SourceContext> context(
      static_cast<SyncFenceMerge::Impl::SourceContext*>(opaque));
  if (context == nullptr || context->impl == nullptr) return;
  std::shared_ptr<SyncFenceMerge::Impl> impl = std::move(context->impl);
  std::lock_guard lock(impl->mutex);
  CloseIfOwned(&impl->input_fds[context->index]);
}

void SyncFenceMerge::Impl::AbortCreate() noexcept {
  std::shared_ptr<void> retained;
  {
    std::lock_guard lock(mutex);
    if (cancelled) return;
    cancelled = true;
    CloseIfOwned(&input_fds[0]);
    CloseIfOwned(&input_fds[1]);
    CloseIfOwned(&output_fd);
    retained = std::move(retained_inputs);
  }
  retained.reset();
}

void SyncFenceMerge::Impl::Cancel() noexcept {
  std::lock_guard cancellation(cancellation_mutex);
  std::array<dispatch_source_t, 2> sources_to_cancel{nullptr, nullptr};
  int output = -1;
  std::shared_ptr<void> retained;
  {
    std::lock_guard lock(mutex);
    if (!cancelled) cancelled = true;
    output = std::exchange(output_fd, -1);
    retained = std::move(retained_inputs);
    sources_to_cancel = std::exchange(sources, {nullptr, nullptr});
  }

  // The output is never written after this point.  Input descriptors remain
  // owned by their source cancellation handlers until dispatch has drained.
  if (output >= 0) (void)close(output);
  CancelSources(sources_to_cancel);
  // Release retained upstream broker ownership only after cancellation has
  // been requested, and never while holding the state mutex.
  retained.reset();
}

void SyncFenceMerge::Impl::CancelSources(
    std::array<dispatch_source_t, 2> sources_to_cancel) noexcept {
  for (dispatch_source_t source : sources_to_cancel) {
    if (source == nullptr) continue;
    dispatch_source_cancel(source);
    // The source retains its context until the cancel handler runs.  Releasing
    // this owner reference here breaks the source/context/Impl cycle while
    // preserving callback lifetime.
    dispatch_release(source);
  }
}

void SyncFenceMerge::Impl::MarkReady(size_t index, bool input_failed) noexcept {
  if (index >= ready.size()) return;
  bool cancel_sources = false;
  bool completed = false;
  std::array<dispatch_source_t, 2> sources_to_cancel{nullptr, nullptr};
  {
    std::lock_guard lock(mutex);
    if (cancelled || signalled) return;
    ready[index] = true;
    failed = failed || input_failed;
    // Read sources are level-triggered.  Once one input is ready, stop
    // observing it immediately or it will spin the shared serial queue while
    // waiting for the other dependency.
    sources_to_cancel[index] = std::exchange(sources[index], nullptr);
    if (!ready[0] || !ready[1]) {
      cancel_sources = true;
    } else {
      // The current event's source is already captured above; the other
      // source may still be live.  Do not overwrite the captured handle.
      for (size_t source_index = 0; source_index != sources.size();
           ++source_index) {
        if (sources_to_cancel[source_index] == nullptr)
          sources_to_cancel[source_index] =
              std::exchange(sources[source_index], nullptr);
      }
    }

    if (ready[0] && ready[1]) {
      completed = true;
      // sync_file_info in the adapter expects one complete uint64_t marker.
      // Keep the write under the same lock as cancellation so Cancel cannot
      // close this descriptor between the readiness decision and the write.
      constexpr uint64_t kSignalledFence = UINT64_C(1);
      const uint8_t* bytes =
          reinterpret_cast<const uint8_t*>(&kSignalledFence);
      size_t written = 0;
      while (!failed && written != sizeof(kSignalledFence)) {
        const ssize_t result = write(output_fd, bytes + written,
                                     sizeof(kSignalledFence) - written);
        if (result > 0) {
          written += static_cast<size_t>(result);
          continue;
        }
        if (result < 0 && errno == EINTR) continue;
        break;
      }
      const int output = std::exchange(output_fd, -1);
      if (output >= 0) (void)close(output);
      signalled = written == sizeof(kSignalledFence);
      // A failed output write cannot be retried safely after ownership has
      // been relinquished.  Tear down the input observers without fabricating
      // a successful fence.
      cancel_sources = true;
    }
  }
  if (cancel_sources) {
    if (sources_to_cancel[0] != nullptr || sources_to_cancel[1] != nullptr)
      CancelSources(sources_to_cancel);
    if (completed) Cancel();
  }
}

SyncFenceMerge::SyncFenceMerge(std::shared_ptr<Impl> impl) noexcept
    : impl_(std::move(impl)) {}

std::shared_ptr<SyncFenceMerge> SyncFenceMerge::Create(
    int owned_first_host_fd, int owned_second_host_fd,
    int owned_output_write_fd, std::shared_ptr<void> retained_inputs) noexcept {
  // The API consumes all three values even when they are invalid or aliases.
  // Aliases are closed exactly once because they denote one kernel object.
  if (owned_first_host_fd < 0 || owned_second_host_fd < 0 ||
      owned_output_write_fd < 0 || owned_first_host_fd == owned_second_host_fd ||
      owned_first_host_fd == owned_output_write_fd ||
      owned_second_host_fd == owned_output_write_fd) {
    CloseDistinct(owned_first_host_fd, owned_second_host_fd,
                  owned_output_write_fd);
    return nullptr;
  }

  if (!SetCloseOnExec(owned_first_host_fd) ||
      !SetCloseOnExec(owned_second_host_fd) ||
      !SetCloseOnExec(owned_output_write_fd) ||
      !SetNoSigPipe(owned_output_write_fd)) {
    CloseDistinct(owned_first_host_fd, owned_second_host_fd,
                  owned_output_write_fd);
    return nullptr;
  }

  std::shared_ptr<Impl> impl;
  try {
    impl = std::make_shared<Impl>(owned_first_host_fd, owned_second_host_fd,
                                  owned_output_write_fd,
                                  std::move(retained_inputs));
  } catch (...) {
    CloseDistinct(owned_first_host_fd, owned_second_host_fd,
                  owned_output_write_fd);
    return nullptr;
  }

  dispatch_queue_t queue = SharedMergeQueue();
  if (queue == nullptr) {
    impl->AbortCreate();
    return nullptr;
  }

  // Allocate the public owner before any source is resumed.  Thus a
  // no-memory failure cannot strand live dispatch contexts with no owner able
  // to request cancellation.
  std::shared_ptr<SyncFenceMerge> owner;
  try {
    owner = std::shared_ptr<SyncFenceMerge>(new SyncFenceMerge(impl));
  } catch (...) {
    impl->AbortCreate();
    return nullptr;
  }

  std::array<SyncFenceMerge::Impl::SourceContext*, 2> contexts{nullptr,
                                                                nullptr};
  for (size_t index = 0; index != impl->sources.size(); ++index) {
    const int input_fd = index == 0 ? owned_first_host_fd
                                    : owned_second_host_fd;
    impl->sources[index] = dispatch_source_create(
        DISPATCH_SOURCE_TYPE_READ, static_cast<uintptr_t>(input_fd), 0,
        queue);
    if (impl->sources[index] == nullptr) {
      for (size_t source_index = 0; source_index != impl->sources.size();
           ++source_index) {
        dispatch_source_t& source = impl->sources[source_index];
        if (source != nullptr) {
          CancelUnconfiguredSource(source, &impl->input_fds[source_index]);
          source = nullptr;
        }
      }
      impl->AbortCreate();
      return nullptr;
    }
  }

  // Allocate every context before installing any one of them.  A source with
  // a cancel handler owns its context; no context is ever manually freed
  // after that handler is installed.
  for (size_t index = 0; index != contexts.size(); ++index) {
    contexts[index] = new (std::nothrow) SyncFenceMerge::Impl::SourceContext{
        impl, index};
    if (contexts[index] == nullptr) {
      for (SyncFenceMerge::Impl::SourceContext* context : contexts)
        delete context;
      for (size_t source_index = 0; source_index != impl->sources.size();
           ++source_index) {
        dispatch_source_t& source = impl->sources[source_index];
        if (source != nullptr) {
          CancelUnconfiguredSource(source, &impl->input_fds[source_index]);
          source = nullptr;
        }
      }
      impl->AbortCreate();
      return nullptr;
    }
  }
  for (size_t index = 0; index != contexts.size(); ++index) {
    dispatch_set_context(impl->sources[index], contexts[index]);
    dispatch_source_set_event_handler_f(
        impl->sources[index], &SyncFenceMerge::Impl::SourceEventHandler);
    dispatch_source_set_cancel_handler_f(impl->sources[index],
                                         &SyncFenceMerge::Impl::SourceCancelHandler);
  }

  // Resume only after both source/context pairs are complete.  From here on,
  // cancellation handlers own both input descriptors and their contexts.
  {
    std::lock_guard lock(impl->mutex);
    dispatch_resume(impl->sources[0]);
    dispatch_resume(impl->sources[1]);
  }
  return owner;
}

void SyncFenceMerge::Cancel() noexcept {
  if (impl_ != nullptr) impl_->Cancel();
}

}  // namespace darwin_art::socket
