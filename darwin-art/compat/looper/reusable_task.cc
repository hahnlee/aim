#include "reusable_task.h"

#include <android/looper.h>

#include <exception>
#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::looper {

struct ReusableLooperTask::State {
  State(void* owner_looper, detail::ReusableTaskQueue* owner_queue,
        const ReusableLooperTaskCallbacks& owner_callbacks)
      : looper(owner_looper), queue(owner_queue), callbacks(owner_callbacks) {}

  // Mutable fields and intrusive links are protected by queue->mutex.
  // `looper`, `queue`, and callbacks are immutable after Prepare.
  void* const looper;
  detail::ReusableTaskQueue* const queue;
  const ReusableLooperTaskCallbacks callbacks;
  bool closed = false;
  bool queued = false;
  bool running = false;
  bool requested = false;
  bool wake_pending = false;
  bool quiescence_reported = false;
  uint64_t queue_generation = 0;
  std::shared_ptr<State> next;
  State* prev = nullptr;
};

}  // namespace darwin_art::looper

namespace darwin_art::looper::detail {

struct ReusableTaskQueue {
  std::mutex mutex;
  std::shared_ptr<ReusableLooperTask::State> head;
  ReusableLooperTask::State* tail = nullptr;
  uint64_t next_generation = 1;
};

ReusableTaskQueue* CreateReusableTaskQueue() {
  return new (std::nothrow) ReusableTaskQueue();
}

void DestroyReusableTaskQueue(ReusableTaskQueue* queue) { delete queue; }

namespace {

void AppendLocked(ReusableTaskQueue* queue,
                  const std::shared_ptr<ReusableLooperTask::State>& state) {
  state->queued = true;
  state->queue_generation = queue->next_generation++;
  state->prev = queue->tail;
  state->next.reset();
  if (queue->tail != nullptr)
    queue->tail->next = state;
  else
    queue->head = state;
  queue->tail = state.get();
}

void UnlinkLocked(ReusableTaskQueue* queue,
                  ReusableLooperTask::State* state) {
  if (!state->queued) return;
  // The caller owns a separate strong pin. Move the successor before
  // changing links so no task destructor can run under queue->mutex.
  std::shared_ptr<ReusableLooperTask::State> next = std::move(state->next);
  auto* previous = state->prev;
  if (previous != nullptr)
    previous->next = std::move(next);
  else
    queue->head = std::move(next);
  if (previous != nullptr && previous->next != nullptr)
    previous->next->prev = previous;
  else if (previous == nullptr && queue->head != nullptr)
    queue->head->prev = nullptr;
  else
    queue->tail = previous;
  if (queue->head == nullptr) queue->tail = nullptr;
  state->prev = nullptr;
  state->queued = false;
}

std::shared_ptr<ReusableLooperTask::State> PopLocked(ReusableTaskQueue* queue,
                                                      uint64_t cutoff) {
  if (queue->head == nullptr || queue->head->queue_generation > cutoff)
    return {};
  // Retain this pin before replacing the owning head. If the external handle
  // was dropped, the callback still has a valid state until it completes.
  auto state = std::move(queue->head);
  auto next = std::move(state->next);
  queue->head = std::move(next);
  if (queue->head != nullptr)
    queue->head->prev = nullptr;
  else
    queue->tail = nullptr;
  state->prev = nullptr;
  state->queued = false;
  state->running = true;
  state->requested = false;
  return state;
}

thread_local uint32_t g_dispatch_depth = 0;

void PublishQuiescence(
    const std::shared_ptr<ReusableLooperTask::State>& state) {
  {
    std::lock_guard lock(state->queue->mutex);
    if (!state->closed || state->queued || state->running ||
        state->quiescence_reported)
      return;
    state->quiescence_reported = true;
  }
  // The caller's state pin retains the immutable context owner. Only metadata
  // follows this point; no task invocation can follow. A concurrent Request's
  // already-published provider wake is only a stale hint, never task admission.
  if (state->callbacks.on_quiescent != nullptr)
    state->callbacks.on_quiescent(state->callbacks.context);
}

}  // namespace

std::shared_ptr<ReusableLooperTask::State> PrepareReusableTaskState(
    void* looper, const ReusableLooperTaskCallbacks& callbacks) {
  if (looper == nullptr || callbacks.callback == nullptr) return {};
  auto* queue = ReusableTaskQueueForLooper(looper);
  if (queue == nullptr) return {};
  try {
    // State and its immutable callback owner are complete before publication.
    return std::make_shared<ReusableLooperTask::State>(looper, queue,
                                                        callbacks);
  } catch (const std::bad_alloc&) {
    return {};
  }
}

bool RequestReusableTask(
    const std::shared_ptr<ReusableLooperTask::State>& state) {
  if (state == nullptr || state->queue == nullptr) return false;
  bool wake = false;
  uint64_t wake_generation = 0;
  {
    std::lock_guard<std::mutex> lock(state->queue->mutex);
    if (state->closed) return false;
    state->requested = true;
    if (!state->queued && !state->running) {
      AppendLocked(state->queue, state);
      state->wake_pending = true;
      wake = true;
      wake_generation = state->queue_generation;
    } else if (state->queued && state->wake_pending) {
      // A previous provider wake may have failed. Coalescing requests retry
      // that durable signal without allocating or publishing a duplicate.
      wake = true;
      wake_generation = state->queue_generation;
    }
  }
  if (!wake) return true;
  const int wake_result = SignalWake(state->looper);
  if (wake_result == 0) {
    std::lock_guard<std::mutex> lock(state->queue->mutex);
    if (state->queued && state->queue_generation == wake_generation)
      state->wake_pending = false;
    return true;
  }
  return false;
}

bool CancelReusableTask(
    const std::shared_ptr<ReusableLooperTask::State>& state) {
  if (state == nullptr || state->queue == nullptr) return true;
  bool wake = false;
  {
    std::lock_guard<std::mutex> lock(state->queue->mutex);
    if (state->closed) return true;
    state->closed = true;
    state->requested = false;
    if (state->queued) {
      UnlinkLocked(state->queue, state.get());
      state->wake_pending = false;
      wake = true;
    }
  }
  if (wake) (void)SignalWake(state->looper);
  PublishQuiescence(state);
  return true;
}

bool IsReusableTaskQuiescent(
    const std::shared_ptr<ReusableLooperTask::State>& state) {
  if (state == nullptr || state->queue == nullptr) return true;
  std::lock_guard<std::mutex> lock(state->queue->mutex);
  return state->closed && !state->queued && !state->running;
}

int DispatchReusableTasks(ReusableTaskQueue* queue) {
  if (queue == nullptr || g_dispatch_depth != 0) return 0;
  uint64_t cutoff = 0;
  {
    std::lock_guard<std::mutex> lock(queue->mutex);
    cutoff = queue->next_generation - 1;
  }
  constexpr int kBudget = 8;
  int dispatched = 0;
  ++g_dispatch_depth;
  struct DepthGuard {
    ~DepthGuard() { --g_dispatch_depth; }
  } depth_guard;
  for (int i = 0; i < kBudget; ++i) {
    std::shared_ptr<ReusableLooperTask::State> state;
    {
      std::lock_guard<std::mutex> lock(queue->mutex);
      state = PopLocked(queue, cutoff);
    }
    if (state == nullptr) break;
    ++dispatched;
    std::exception_ptr failure;
    try {
      state->callbacks.callback(state->callbacks.context);
    } catch (...) {
      failure = std::current_exception();
    }
    bool requeue = false;
    uint64_t requeue_generation = 0;
    {
      std::lock_guard<std::mutex> lock(queue->mutex);
      state->running = false;
      if (failure != nullptr) {
        state->closed = true;
        state->requested = false;
      } else if (state->closed) {
        state->requested = false;
      } else if (state->requested) {
        AppendLocked(queue, state);
        state->wake_pending = true;
        requeue = true;
        requeue_generation = state->queue_generation;
      }
    }
    // Requeue generation is beyond this dispatch cutoff and runs on a later
    // owner poll. Wake only after publication and mutex release.
    if (requeue) {
      if (SignalWake(state->looper) == 0) {
        std::lock_guard<std::mutex> lock(queue->mutex);
        if (state->queued && state->queue_generation == requeue_generation)
          state->wake_pending = false;
      }
    }
    PublishQuiescence(state);
    if (failure != nullptr) std::rethrow_exception(failure);
  }
  return dispatched;
}

}  // namespace darwin_art::looper::detail

namespace darwin_art::looper {

std::shared_ptr<ReusableLooperTask> ReusableLooperTask::Prepare(
    void* looper, const ReusableLooperTaskCallbacks& callbacks) {
  if (looper == nullptr || callbacks.callback == nullptr) return {};
  try {
    // Finish task and handle allocation before any publication. All subsequent
    // queue transitions belong to this module, not the ALooper ABI adapter.
    auto state = detail::PrepareReusableTaskState(looper, callbacks);
    if (state == nullptr) return {};
    struct SharedTask final : ReusableLooperTask {
      explicit SharedTask(std::shared_ptr<State> prepared)
          : ReusableLooperTask(std::move(prepared)) {}
    };
    // One allocation constructs the task and its shared ownership together.
    // A failed control-block allocation must not destroy an unpublished task
    // and invoke cancellation metadata before Prepare returned its handle.
    return std::make_shared<SharedTask>(std::move(state));
  } catch (const std::bad_alloc&) {
    return {};
  }
}

std::shared_ptr<ReusableLooperTask> ReusableLooperTask::Prepare(
    void* looper, TimedTaskCallback callback, void* context,
    std::shared_ptr<void> context_owner) {
  ReusableLooperTaskCallbacks callbacks{callback, context,
                                        std::move(context_owner)};
  return Prepare(looper, callbacks);
}

ReusableLooperTask::ReusableLooperTask(std::shared_ptr<State> state)
    : state_(std::move(state)) {}
ReusableLooperTask::~ReusableLooperTask() { (void)Cancel(); }
bool ReusableLooperTask::Request() {
  const auto state = state_;
  return detail::RequestReusableTask(state);
}
bool ReusableLooperTask::Cancel() {
  const auto state = state_;
  return detail::CancelReusableTask(state);
}
bool ReusableLooperTask::IsQuiescent() const {
  const auto state = state_;
  return detail::IsReusableTaskQuiescent(state);
}

}  // namespace darwin_art::looper
