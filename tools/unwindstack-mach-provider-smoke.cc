#include <atomic>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <thread>
#include <ucontext.h>
#include <sys/wait.h>
#include <unistd.h>

#include <unwindstack/Maps.h>
#include <unwindstack/Memory.h>
#include <unwindstack/AndroidUnwinder.h>
#include <unwindstack/Demangle.h>
#include <unwindstack/Regs.h>
#include <unwindstack/RegsGetLocal.h>

__attribute__((noinline)) static size_t CaptureLocalFramesLeaf() {
  unwindstack::AndroidLocalUnwinder unwinder;
  unwindstack::AndroidUnwinderData data;
  return unwinder.Unwind(data) ? data.frames.size() : 0;
}

__attribute__((noinline)) static size_t CaptureLocalFramesMiddle() {
  const size_t frames = CaptureLocalFramesLeaf();
  asm volatile("" : : "r"(frames) : "memory");
  return frames;
}

__attribute__((noinline)) static size_t CaptureLocalFrames() {
  const size_t frames = CaptureLocalFramesMiddle();
  asm volatile("" : : "r"(frames) : "memory");
  return frames;
}

__attribute__((noinline)) static void HoldWorker(std::atomic<bool>* stop_worker) {
  while (!stop_worker->load(std::memory_order_acquire)) std::this_thread::yield();
  asm volatile("" : : "r"(stop_worker) : "memory");
}

__attribute__((noinline)) static void RunWorker(std::atomic<uint64_t>* worker_id,
                                                std::atomic<bool>* stop_worker) {
  uint64_t current_id = 0;
  pthread_threadid_np(nullptr, &current_id);
  worker_id->store(current_id, std::memory_order_release);
  HoldWorker(stop_worker);
  asm volatile("" : : "r"(worker_id) : "memory");
}

__attribute__((noinline)) static void WaitInRemoteChild(int ready_fd, int release_fd) {
  char value = 'R';
  if (write(ready_fd, &value, 1) != 1) _exit(90);
  if (read(release_fd, &value, 1) != 1) _exit(91);
  asm volatile("" : : "r"(release_fd) : "memory");
}

__attribute__((noinline)) static void RunRemoteChild(int ready_fd, int release_fd) {
  WaitInRemoteChild(ready_fd, release_fd);
  asm volatile("" : : "r"(ready_fd) : "memory");
}

int main() {
  constexpr uint64_t kValue = 0x1020304050607080ULL;
  auto memory = unwindstack::Memory::CreateProcessMemoryThreadCached(getpid());
  uint64_t memory_value = 0;
  if (memory == nullptr ||
      !memory->ReadFully(reinterpret_cast<uint64_t>(&kValue), &memory_value,
                         sizeof(memory_value)) ||
      memory_value != kValue) {
    std::fprintf(stderr, "Mach process-memory read failed\n");
    return 1;
  }

  unwindstack::LocalUpdatableMaps maps;
  if (!maps.Parse()) {
    std::fprintf(stderr, "dyld/Mach map discovery failed\n");
    return 2;
  }
  auto map = maps.Find(reinterpret_cast<uint64_t>(&main));
  if (map == nullptr || map->name().empty()) {
    std::fprintf(stderr, "main executable map was not named\n");
    return 3;
  }

  std::unique_ptr<unwindstack::Regs> local(unwindstack::Regs::CreateFromLocal());
  unwindstack::RegsGetLocal(local.get());
  if (local->Arch() != unwindstack::ARCH_ARM64 || local->pc() == 0 || local->sp() == 0) {
    std::fprintf(stderr, "local ARM64 register capture failed\n");
    return 4;
  }

  unwindstack::ErrorCode error = unwindstack::ERROR_NONE;
  std::unique_ptr<unwindstack::Regs> sampled(unwindstack::Regs::RemoteGet(getpid(), &error));
  if (sampled == nullptr || sampled->Arch() != unwindstack::ARCH_ARM64 || sampled->pc() == 0 ||
      sampled->sp() == 0) {
    std::fprintf(stderr, "Mach thread-state capture failed: %u\n", error);
    return 5;
  }

  const size_t frames = CaptureLocalFrames();
  if (frames < 3) {
    std::fprintf(stderr, "AOSP AndroidLocalUnwinder returned only %zu frames\n", frames);
    return 6;
  }

  ucontext_t context{};
  getcontext(&context);
  unwindstack::AndroidLocalUnwinder context_unwinder;
  unwindstack::AndroidUnwinderData context_data;
  if (!context_unwinder.Unwind(&context, context_data) || context_data.frames.size() < 2) {
    std::fprintf(stderr, "Mach ucontext unwind returned only %zu frames\n",
                 context_data.frames.size());
    return 7;
  }

  std::atomic<uint64_t> worker_id{0};
  std::atomic<bool> stop_worker{false};
  std::thread worker(RunWorker, &worker_id, &stop_worker);
  while (worker_id.load(std::memory_order_acquire) == 0) std::this_thread::yield();
  unwindstack::AndroidLocalUnwinder thread_unwinder;
  unwindstack::AndroidUnwinderData thread_data;
  const bool thread_ok = thread_unwinder.Unwind(
      static_cast<pid_t>(worker_id.load(std::memory_order_acquire)), thread_data);
  stop_worker.store(true, std::memory_order_release);
  worker.join();
  if (!thread_ok || thread_data.frames.size() < 2) {
    std::fprintf(stderr, "Mach other-thread unwind returned only %zu frames\n",
                 thread_data.frames.size());
    return 8;
  }

  int ready_pipe[2]{};
  int release_pipe[2]{};
  if (pipe(ready_pipe) != 0 || pipe(release_pipe) != 0) return 9;
  const pid_t child = fork();
  if (child == 0) {
    close(ready_pipe[0]);
    close(release_pipe[1]);
    RunRemoteChild(ready_pipe[1], release_pipe[0]);
    _exit(0);
  }
  close(ready_pipe[1]);
  close(release_pipe[0]);
  char ready = 0;
  if (child < 0 || read(ready_pipe[0], &ready, 1) != 1) return 10;
  unwindstack::AndroidRemoteUnwinder remote_unwinder(child);
  unwindstack::AndroidUnwinderData remote_data;
  const bool remote_ok = remote_unwinder.Unwind(remote_data);
  char release = 'X';
  write(release_pipe[1], &release, 1);
  int child_status = 0;
  waitpid(child, &child_status, 0);
  close(ready_pipe[0]);
  close(release_pipe[1]);
  bool named_remote_frame = false;
  for (const auto& frame : remote_data.frames) {
    if (frame.map_info != nullptr && !frame.map_info->name().empty()) named_remote_frame = true;
  }
  const bool remote_unavailable =
      !remote_ok && remote_data.frames.empty() &&
      (remote_data.error.code == unwindstack::ERROR_PTRACE_CALL ||
       remote_data.error.code == unwindstack::ERROR_MAPS_PARSE);
  if ((!remote_ok || remote_data.frames.size() < 2 || !named_remote_frame) &&
      !remote_unavailable) {
    std::fprintf(stderr, "Mach remote unwind frames=%zu named=%d child_status=%d error=%u\n",
                 remote_data.frames.size(), named_remote_frame, child_status,
                 remote_data.error.code);
    return 11;
  }
  if (!WIFEXITED(child_status) || WEXITSTATUS(child_status) != 0) return 11;
  if (remote_unavailable) {
    std::fprintf(stderr,
                 "Mach remote unwind unavailable: task access denied by host; "
                 "in-process unwind checks remain strict\n");
  }

  if (unwindstack::DemangleNameIfNeeded(
          "_RNvNtCs2WRBrrl1bb1_3std2rt19lang_start_internal") !=
          "std::rt::lang_start_internal" ||
      unwindstack::DemangleNameIfNeeded("_RNvCs4VPobU5SDH_12profcollectd4main") !=
          "profcollectd::main") {
    std::fprintf(stderr, "Rust v0 symbol demangling failed\n");
    return 12;
  }

  std::printf(
      "unwindstack-mach-provider: memory=pass maps=%zu regs=pass frames=%zu context_frames=%zu "
      "thread_frames=%zu remote_frames=%zu rust_demangle=pass image=%s\n",
      maps.Total(), frames, context_data.frames.size(), thread_data.frames.size(),
      remote_unavailable ? 0 : remote_data.frames.size(), map->name().c_str());
  return 0;
}
