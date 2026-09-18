#include "routing_transport_scheduler.h"

#include <mutex>
#include <new>
#include <utility>

namespace darwin_art::input {

struct RoutingTransportScheduler::State {
  explicit State(void* owner_looper, InputRoutingHandle owner_routing,
                 const RoutingTransportSchedulerCallbacks& owner_callbacks)
      : looper(owner_looper),
        routing(std::move(owner_routing)),
        callbacks(owner_callbacks) {}

  mutable std::mutex mutex;
  const void* looper = nullptr;
  const InputRoutingHandle routing;
  const RoutingTransportSchedulerCallbacks callbacks;
  bool closed = false;
  bool running = false;
  bool pending = false;
  bool scheduled = false;
  uint64_t next_generation = 1;
  uint64_t scheduled_generation = 0;
};

namespace {

struct ScheduledTask {
  std::shared_ptr<RoutingTransportScheduler::State> state;
  uint64_t generation = 0;
};

void ReleaseTask(void* opaque) {
  delete static_cast<ScheduledTask*>(opaque);
}

void NotifyFailure(
    const std::shared_ptr<RoutingTransportScheduler::State>& state) noexcept {
  RoutingTransportSchedulerCallbacks callbacks;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    callbacks = state->callbacks;
  }
  if (callbacks.on_failure == nullptr) return;
  try {
    callbacks.on_failure(callbacks.context);
  } catch (...) {
    // Failure reporting must not escape the looper/provider boundary.
  }
}

bool MarkScheduleFailure(const std::shared_ptr<RoutingTransportScheduler::State>& state) {
  std::lock_guard<std::mutex> lock(state->mutex);
  state->scheduled = false;
  state->pending = !state->closed;
  return !state->closed;
}

void RunTask(void* opaque) {
  auto* task = static_cast<ScheduledTask*>(opaque);
  if (task == nullptr || task->state == nullptr) return;
  const auto state = task->state;
  RoutingTransportSchedulerCallbacks callbacks;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed || !state->scheduled ||
        state->scheduled_generation != task->generation)
      return;
    state->running = true;
    state->pending = false;
    callbacks = state->callbacks;
  }
  RoutingTransportDrainResult result;
  try {
    if (callbacks.on_progress != nullptr)
      result = callbacks.on_progress(callbacks.context);
  } catch (...) {
    {
      std::lock_guard<std::mutex> lock(state->mutex);
      state->running = false;
      state->pending = false;
      state->scheduled = false;
      state->closed = true;
    }
    // Notify outside the scheduler mutex while the state is still retained;
    // progress failures must not escape the looper/provider boundary.
    NotifyFailure(state);
    return;
  }

  bool schedule = false;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    state->running = false;
    if (state->closed) {
      state->pending = false;
      state->scheduled = false;
    } else if (result.continuation_needed || state->pending) {
      state->pending = false;
      schedule = true;
      state->scheduled_generation = state->next_generation++;
      state->scheduled = true;
    } else {
      state->scheduled = false;
    }
  }
  if (!schedule) return;

  auto* next = new (std::nothrow) ScheduledTask;
  if (next == nullptr) {
    if (MarkScheduleFailure(state)) NotifyFailure(state);
    return;
  }
  next->state = state;
  next->generation = state->scheduled_generation;
  const int64_t deadline = 0;
  int scheduled = 0;
  try {
    scheduled = darwin_art::looper::ScheduleTimedTaskAt(
        const_cast<void*>(state->looper), deadline, &RunTask, next,
        &ReleaseTask);
  } catch (...) {
    // ScheduleTimedTaskAt consumes and releases the task on rejected enqueue,
    // including its consume-and-throw failure path.
    if (MarkScheduleFailure(state)) NotifyFailure(state);
    return;
  }
  if (scheduled != 1) {
    // The timed-task provider releases next on a rejected enqueue.
    if (MarkScheduleFailure(state)) NotifyFailure(state);
  }
}

bool ScheduleLockedState(const std::shared_ptr<RoutingTransportScheduler::State>& state,
                         uint64_t generation) {
  auto* task = new (std::nothrow) ScheduledTask;
  if (task == nullptr) return false;
  task->state = state;
  task->generation = generation;
  int scheduled = 0;
  try {
    scheduled = darwin_art::looper::ScheduleTimedTaskAt(
        const_cast<void*>(state->looper), 0, &RunTask, task, &ReleaseTask);
  } catch (...) {
    // The provider owns failure cleanup, even when its release callback then
    // propagates an exception.
    return false;
  }
  if (scheduled != 1) return false;
  return true;
}

}  // namespace

std::shared_ptr<RoutingTransportScheduler> RoutingTransportScheduler::Create(
    void* looper, InputRoutingHandle routing,
    const RoutingTransportSchedulerCallbacks& callbacks) {
  if (looper == nullptr || callbacks.on_progress == nullptr) return {};
  try {
    return std::shared_ptr<RoutingTransportScheduler>(new RoutingTransportScheduler(
        std::make_shared<State>(looper, std::move(routing), callbacks)));
  } catch (const std::bad_alloc&) {
    return {};
  }
}

RoutingTransportScheduler::RoutingTransportScheduler(std::shared_ptr<State> state)
    : state_(std::move(state)) {}

RoutingTransportScheduler::~RoutingTransportScheduler() { (void)Retire(); }

bool RoutingTransportScheduler::Request() {
  const auto state = state_;
  if (state == nullptr) return false;
  uint64_t generation = 0;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->closed) return false;
    state->pending = true;
    if (state->scheduled || state->running) return true;
    state->scheduled = true;
    generation = state->scheduled_generation = state->next_generation++;
  }
  if (ScheduleLockedState(state, generation)) return true;
  if (MarkScheduleFailure(state)) NotifyFailure(state);
  return false;
}

bool RoutingTransportScheduler::Retire() {
  const auto state = state_;
  if (state == nullptr) return true;
  {
    std::lock_guard<std::mutex> lock(state->mutex);
    state->closed = true;
    state->pending = false;
    // Timed tasks cannot be removed from the provider. They retain State and
    // will observe closed before calling policy; the task release then drops
    // its final pin without touching the scheduler object.
  }
  return true;
}

}  // namespace darwin_art::input
