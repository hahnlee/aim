#include "darwin_jit_memory.h"
#include <atomic>
#include <cassert>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <sys/mman.h>
#include <thread>
#include <unistd.h>

int main(int argc, char** argv) {
  // The Android 16 boot image plus a Chromium process exceeds the historical
  // 16K fault-method table.  Publish well beyond that boundary before testing
  // MAP_JIT so the signal-safe registry cannot regress to the smaller size.
  constexpr uintptr_t kRegistryBase = UINT64_C(0x100000000);
  constexpr uintptr_t kRegistryStride = UINT64_C(0x2000);
  constexpr size_t kRegistryEntries = 32768;
  for (size_t index = 0; index < kRegistryEntries; ++index) {
    const uintptr_t code = kRegistryBase + index * kRegistryStride;
    const uintptr_t method = UINT64_C(0x400000000) + index * 8;
    DarwinArtRegisterJitMethod(code, 64, method);
  }
  for (size_t index : {size_t{0}, size_t{16383}, size_t{16384},
                       kRegistryEntries - 1}) {
    const uintptr_t code = kRegistryBase + index * kRegistryStride;
    const uintptr_t method = UINT64_C(0x400000000) + index * 8;
    assert(DarwinArtLookupJitCode(code + 32));
    assert(DarwinArtLookupJitMethod(code + 32) == method);
  }

  const size_t size = static_cast<size_t>(getpagesize());
  void* code = DarwinArtMapJitCode(size);
  if (code == MAP_FAILED) { perror("MAP_JIT"); return 1; }
  const uint32_t first[] = {0x52800540, 0xd65f03c0}; // mov w0,#42; ret
  {
    DarwinArtJitWriteScope outer;
    { DarwinArtJitWriteScope inner; memcpy(code, first, sizeof(first)); }
    memcpy(static_cast<char*>(code) + 64, first, sizeof(first));
    DarwinArtFlushJitCode(code, 72);
  }
  auto first_fn = reinterpret_cast<int (*)()>(code);
  assert(first_fn() == 42);
  if (argc == 2 && strcmp(argv[1], "--protected-write") == 0) {
    fprintf(stderr, "JIT memory negative phase=protected-write armed address=%p\n",
            code);
    *static_cast<volatile uint32_t*>(code) = first[0];
    return 99; // Must fault: this thread is execute-only for JIT memory.
  }
  if (argc == 2 && strcmp(argv[1], "--execute-while-writing") == 0) {
    DarwinArtJitWriteScope write;
    fprintf(stderr,
            "JIT memory negative phase=execute-while-writing armed address=%p\n",
            code);
    return first_fn() == 42 ? 99 : 98; // Must fault: this thread cannot execute.
  }
  std::atomic<bool> done{false};
  std::atomic<size_t> executions{0};
  std::thread reader([&] {
    while (!done.load(std::memory_order_acquire)) {
      assert(first_fn() == 42);
      executions.fetch_add(1, std::memory_order_relaxed);
    }
  });
  while (executions.load(std::memory_order_relaxed) == 0) std::this_thread::yield();
  const uint32_t second[] = {0x52800920, 0xd65f03c0}; // return 73
  {
    DarwinArtJitWriteScope write;
    const size_t before = executions.load();
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(5);
    while (executions.load() < before + 1000) {
      assert(std::chrono::steady_clock::now() < deadline);
      std::this_thread::yield();
    }
    memcpy(static_cast<char*>(code) + 64, second, sizeof(second));
    DarwinArtFlushJitCode(static_cast<char*>(code) + 64, sizeof(second));
  }
  assert(reinterpret_cast<int (*)()>(static_cast<char*>(code) + 64)() == 73);
  done.store(true, std::memory_order_release);
  reader.join();
  assert(munmap(code, size) == 0);
  printf("JIT memory PASS: registry=%zu nested thread-local W^X, concurrent execution=%zu\n",
         kRegistryEntries, executions.load());
}
