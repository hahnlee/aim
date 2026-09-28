#include "input-owner-task-fixture.h"
#include "compat/looper/reusable_task.h"

#include <thread>

namespace {
const std::thread::id owner_thread = std::this_thread::get_id();
void* const owner_looper = reinterpret_cast<void*>(1);
aim::looper::detail::ReusableTaskQueue* OwnerQueue() {
  static auto* queue = aim::looper::detail::CreateReusableTaskQueue();
  return queue;
}
}

namespace aim::looper {
void* Current() {
  return std::this_thread::get_id() == owner_thread ? owner_looper : nullptr;
}
int SignalWake(void* looper) { return looper == owner_looper ? 0 : 5; }
namespace detail {
ReusableTaskQueue* ReusableTaskQueueForLooper(void* looper) {
  return looper == owner_looper ? OwnerQueue() : nullptr;
}
}
}

namespace aim::test {
int DispatchInputOwnerTasks() {
  return aim::looper::Current() == owner_looper
             ? aim::looper::detail::DispatchReusableTasks(OwnerQueue())
             : 0;
}
}
