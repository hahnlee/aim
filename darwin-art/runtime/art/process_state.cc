#include "process_state.h"

#include <pthread.h>

#include <mutex>
#include <utility>
#include <vector>

#include "../../include/darwin_art/darwin_art.h"

#include "base/locks.h"
#include "runtime.h"
#include "scoped_thread_state_change-inl.h"
#include "thread-current-inl.h"

extern "C" int darwin_art_bionic_process_state_is_installed(void);

namespace darwin_art_process {
namespace {

struct State {
  std::mutex mutex;
  // Rust RuntimeLifecycle is authoritative whenever lifecycle_hooks is
  // present. These booleans are native readiness gates for ART-owned
  // pointers and the legacy null-hook ABI; they intentionally do not mirror
  // Rust's phase machine.
  bool run_started = false;
  bool runtime_created = false;
  bool shutdown_started = false;
  bool shutdown_complete = false;
  bool failed = false;
  pthread_t owner_thread{};
  bool owner_thread_valid = false;
  JavaVM* java_vm = nullptr;
  art::Thread* art_thread = nullptr;
  bool framework_vm_bound = false;
  bool dalvikvm_process = false;
  darwin_art_graphics::GraphicsState* graphics_state = nullptr;
  const darwin_art_lifecycle_hooks_t* lifecycle_hooks = nullptr;
  std::vector<std::unique_ptr<const art::DexFile>> app_dex_files;
};

State g_state;

}  // namespace

bool begin_run(const struct darwin_art_lifecycle_hooks* lifecycle_hooks) {
  // The embedding engine owns the configured snapshot across ART and provider
  // teardown. Runtime entry only borrows it; never install fixture defaults or
  // release the caller's snapshot on an entry failure.
  if (darwin_art_bionic_process_state_is_installed() != 1) {
    return false;
  }
  if (lifecycle_hooks != nullptr) {
    if (lifecycle_hooks->struct_size < sizeof(*lifecycle_hooks) ||
        lifecycle_hooks->abi_version != DARWIN_ART_ABI_VERSION ||
        lifecycle_hooks->context == nullptr ||
        lifecycle_hooks->begin_run == nullptr ||
        lifecycle_hooks->finish_run == nullptr ||
        lifecycle_hooks->begin_shutdown == nullptr ||
        lifecycle_hooks->mark_failed == nullptr ||
        lifecycle_hooks->begin_run(lifecycle_hooks->context) != 0) {
      return false;
    }
  }
  std::lock_guard<std::mutex> lock(g_state.mutex);
  if (g_state.run_started) {
    return false;
  }
  g_state.run_started = true;
  g_state.owner_thread = pthread_self();
  g_state.owner_thread_valid = true;
  g_state.lifecycle_hooks = lifecycle_hooks;
  return true;
}

void record_created_runtime(art::Thread* art_thread) {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  CHECK(g_state.run_started && !g_state.failed && !g_state.shutdown_started);
  CHECK(art::Runtime::Current() != nullptr);
  g_state.java_vm = reinterpret_cast<JavaVM*>(art::Runtime::Current()->GetJavaVM());
  g_state.art_thread = art_thread;
  g_state.runtime_created = true;
}

void record_dalvikvm_process() {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  CHECK(g_state.run_started && g_state.runtime_created && !g_state.failed &&
        !g_state.shutdown_started);
  g_state.dalvikvm_process = true;
}

void record_graphics_state(darwin_art_graphics::GraphicsState* state) {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  CHECK(g_state.run_started && !g_state.failed && !g_state.shutdown_started);
  g_state.graphics_state = state;
}

void record_framework_vm_bound() {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  CHECK(g_state.run_started && !g_state.failed && !g_state.shutdown_started);
  CHECK(!g_state.framework_vm_bound);
  g_state.framework_vm_bound = true;
}

void finish_run(bool runtime_created) {
  const struct darwin_art_lifecycle_hooks* lifecycle_hooks = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_state.mutex);
    CHECK(g_state.run_started && !g_state.failed && !g_state.shutdown_started);
    g_state.runtime_created = runtime_created;
    if (!runtime_created) g_state.failed = true;
    lifecycle_hooks = g_state.lifecycle_hooks;
  }
  if (lifecycle_hooks != nullptr &&
      lifecycle_hooks->finish_run(lifecycle_hooks->context,
                                  runtime_created ? 1 : 0) != 0) {
    std::lock_guard<std::mutex> lock(g_state.mutex);
    g_state.failed = true;
  }
}

void record_app_dex_file(std::unique_ptr<const art::DexFile> dex_file) {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  g_state.app_dex_files.emplace_back(std::move(dex_file));
}

std::vector<std::unique_ptr<const art::DexFile>>& app_dex_files() {
  return g_state.app_dex_files;
}

void clear_app_dex_files() {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  g_state.app_dex_files.clear();
}

art::Thread* owner_thread_for_callback() {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  if (!g_state.runtime_created || g_state.failed || g_state.shutdown_started ||
      !g_state.owner_thread_valid ||
      pthread_equal(g_state.owner_thread, pthread_self()) == 0 ||
      g_state.art_thread == nullptr) {
    return nullptr;
  }
  return g_state.art_thread;
}

darwin_art_graphics::GraphicsState* graphics_state_for_callback() {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  if (!g_state.runtime_created || g_state.failed || g_state.shutdown_started ||
      !g_state.owner_thread_valid ||
      pthread_equal(g_state.owner_thread, pthread_self()) == 0 ||
      g_state.art_thread == nullptr ||
      art::Thread::Current() != g_state.art_thread) {
    return nullptr;
  }
  return g_state.graphics_state;
}

ShutdownBeginResult begin_shutdown(ShutdownSnapshot* snapshot) {
  const struct darwin_art_lifecycle_hooks* lifecycle_hooks = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_state.mutex);
    if (g_state.shutdown_complete) return ShutdownBeginResult::kAlreadyComplete;
    if (g_state.failed) return ShutdownBeginResult::kFailed;
    if (!g_state.run_started || !g_state.runtime_created ||
        g_state.shutdown_started) {
      return ShutdownBeginResult::kNotReady;
    }
    if (!g_state.owner_thread_valid ||
        pthread_equal(g_state.owner_thread, pthread_self()) == 0 ||
        (g_state.art_thread != nullptr &&
         art::Thread::Current() != g_state.art_thread)) {
      return ShutdownBeginResult::kWrongThread;
    }
    g_state.shutdown_started = true;
    snapshot->java_vm = g_state.java_vm;
    snapshot->art_thread = g_state.art_thread;
    snapshot->framework_vm_bound = g_state.framework_vm_bound;
    snapshot->dalvikvm_process = g_state.dalvikvm_process;
    snapshot->graphics_state = g_state.graphics_state;
    lifecycle_hooks = g_state.lifecycle_hooks;
  }
  if (lifecycle_hooks != nullptr &&
      lifecycle_hooks->begin_shutdown(lifecycle_hooks->context) != 0) {
    std::lock_guard<std::mutex> lock(g_state.mutex);
    g_state.failed = true;
    return ShutdownBeginResult::kFailed;
  }
  return ShutdownBeginResult::kReady;
}

ShutdownBeginResult inspect_shutdown(ShutdownSnapshot* snapshot) {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  if (g_state.shutdown_complete) return ShutdownBeginResult::kAlreadyComplete;
  if (g_state.failed) return ShutdownBeginResult::kFailed;
  if (!g_state.run_started || !g_state.runtime_created || g_state.shutdown_started)
    return ShutdownBeginResult::kNotReady;
  if (!g_state.owner_thread_valid ||
      pthread_equal(g_state.owner_thread, pthread_self()) == 0 ||
      (g_state.art_thread != nullptr && art::Thread::Current() != g_state.art_thread))
    return ShutdownBeginResult::kWrongThread;
  snapshot->java_vm = g_state.java_vm;
  snapshot->art_thread = g_state.art_thread;
  snapshot->framework_vm_bound = g_state.framework_vm_bound;
  snapshot->dalvikvm_process = g_state.dalvikvm_process;
  snapshot->graphics_state = g_state.graphics_state;
  return ShutdownBeginResult::kReady;
}

void mark_shutdown_failed() {
  const darwin_art_lifecycle_hooks_t* lifecycle_hooks = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_state.mutex);
    g_state.failed = true;
    lifecycle_hooks = g_state.lifecycle_hooks;
  }
  if (lifecycle_hooks != nullptr) {
    lifecycle_hooks->mark_failed(lifecycle_hooks->context,
                                 DARWIN_ART_STATUS_SHUTDOWN_FAILED);
  }
}

void mark_shutdown_complete() {
  std::lock_guard<std::mutex> lock(g_state.mutex);
  g_state.java_vm = nullptr;
  g_state.art_thread = nullptr;
  g_state.framework_vm_bound = false;
  g_state.dalvikvm_process = false;
  g_state.graphics_state = nullptr;
  g_state.lifecycle_hooks = nullptr;
  g_state.shutdown_complete = true;
}

ScopedRunBoundary::~ScopedRunBoundary() {
  if (art_thread_ == nullptr) {
    finish_run(false);
    return;
  }
  CHECK_EQ(art::Thread::Current(), art_thread_);
  const art::ThreadState state = art_thread_->GetState();
  if (state == art::ThreadState::kRunnable) {
    art_thread_->TransitionFromRunnableToSuspended(art::ThreadState::kNative);
  } else {
    CHECK_EQ(state, art::ThreadState::kNative);
  }
  finish_run(true);
}

void ScopedRunBoundary::set_art_thread(art::Thread* art_thread) {
  DCHECK(art_thread_ == nullptr);
  DCHECK(art_thread != nullptr);
  art_thread_ = art_thread;
}

}  // namespace darwin_art_process
