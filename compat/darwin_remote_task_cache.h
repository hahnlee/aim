#pragma once

#if defined(__APPLE__)

#include <mach/mach.h>

#include <mutex>
#include <condition_variable>
#include <chrono>
#include <thread>
#include <memory>
#include <unordered_map>
#include <unistd.h>
namespace aim::remote_task {

// Keep one validated task send right per remote process.  macOS may block on
// repeated task_for_pid calls while a target is in a fork/exec transition;
// callers receive a temporary send-right copy and remain responsible for
// deallocating that copy.
inline mach_port_t Acquire(pid_t pid) {
  if (pid == getpid()) return mach_task_self();
  static std::mutex mutex;
  static std::unordered_map<pid_t, mach_port_t> cache;
  std::lock_guard<std::mutex> lock(mutex);
  auto found = cache.find(pid);
  if (found == cache.end()) {
    struct Result {
      std::mutex mutex;
      std::condition_variable ready;
      bool done = false;
      mach_port_t task = MACH_PORT_NULL;
      kern_return_t status = KERN_FAILURE;
    };
    auto result = std::make_shared<Result>();
    std::thread([pid, result] {
      mach_port_t task = MACH_PORT_NULL;
      kern_return_t status = KERN_FAILURE;
      // A stopped child can briefly remain in the fork/exec transition from
      // task_for_pid's perspective. Retry while it is still alive instead of
      // turning that transient lookup race into a permanent ESRCH result.
      for (int attempt = 0; attempt != 8; ++attempt) {
        status = task_for_pid(mach_task_self(), pid, &task);
        if (status != KERN_SUCCESS || task == MACH_PORT_NULL) {
          // Newer macOS releases may reject the legacy task_for_pid trap even
          // for a debug-entitled target. The debugger control port is the
          // supported restricted-task equivalent and is sufficient for the
          // stopped-child thread/memory snapshot used by unwindstack.
          mach_port_t debug_task = MACH_PORT_NULL;
          const kern_return_t debug_status =
              debug_control_port_for_pid(mach_task_self(), pid, &debug_task);
          if (debug_status == KERN_SUCCESS && debug_task != MACH_PORT_NULL) {
            status = debug_status;
            task = debug_task;
          }
        }
        if (status == KERN_SUCCESS && task != MACH_PORT_NULL) break;
        if (kill(pid, 0) != 0) break;
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
      }
      {
        std::lock_guard<std::mutex> lock(result->mutex);
        result->status = status;
        result->task = task;
        result->done = true;
      }
      result->ready.notify_one();
    }).detach();
    std::unique_lock<std::mutex> lock(result->mutex);
    if (!result->ready.wait_for(lock, std::chrono::milliseconds(1000),
                                [&] { return result->done; })) {
      return MACH_PORT_NULL;
    }
    if (result->status != KERN_SUCCESS || result->task == MACH_PORT_NULL) {
      return MACH_PORT_NULL;
    }
    mach_port_t task = result->task;
    cache.emplace(pid, task);
    found = cache.find(pid);
  }
  if (mach_port_mod_refs(mach_task_self(), found->second, MACH_PORT_RIGHT_SEND, 1) !=
      KERN_SUCCESS) {
    return MACH_PORT_NULL;
  }
  return found->second;
}

}  // namespace aim::remote_task

#endif
