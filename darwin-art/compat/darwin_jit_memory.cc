#include "darwin_jit_memory.h"
#include <cassert>
#include <atomic>
#include <cerrno>
#include <libkern/OSCacheControl.h>
#include <pthread.h>
#include <cstdio>
#include <cstdlib>
#include <sys/mman.h>

namespace { thread_local unsigned write_depth = 0; }

namespace {
struct JitMethodEntry {
  std::atomic<uintptr_t> start{0};
  std::atomic<uintptr_t> end{0};
  std::atomic<uintptr_t> method{0};
};
// The registry is consulted from ART's signal path and must remain lock-free.
// A boot image can publish several thousand AOT entry ranges before the first
// application method is JIT-compiled; a small fixed table silently dropped
// the application's range and caused implicit-null faults to reach the user
// SIGSEGV handler. Keep ample headroom for boot plus app code while retaining
// the signal-safe, bounded representation.
// Android's boot image already contributes tens of thousands of compiled
// methods before a large application such as Chromium starts publishing app
// AOT and JIT code.  The registry is read from the signal path, so it cannot
// grow or take a lock after publication.  Keep enough fixed capacity for the
// boot, framework, application and JIT ranges instead of silently dropping
// the first app ranges once the old 16K table filled.
constexpr size_t kJitMethodEntries = 131072;
// A non-zero reservation marker keeps signal-path readers from observing a
// slot between its start CAS and the publication of end/method.
constexpr uintptr_t kJitEntryPublishing = 1u;
// ART's quick-code size describes the instruction stream but may omit the
// short signal/epilogue/alignment tail reached by the PC reported on Darwin.
// Keep the published range inclusive of that tail so implicit-null faults are
// recognized as managed code. The AOT code page remains an ART-owned mapping.
constexpr uintptr_t kJitCodeTailBytes = 4096u;
JitMethodEntry g_jit_method_entries[kJitMethodEntries];
}

void DarwinArtRegisterJitMethod(uintptr_t code, size_t size, uintptr_t method) {
  if (code == 0 || size == 0) return;
  uintptr_t end = code > UINTPTR_MAX - size ? UINTPTR_MAX : code + size;
  if (end != UINTPTR_MAX) {
    end = end > UINTPTR_MAX - kJitCodeTailBytes
              ? UINTPTR_MAX
              : end + kJitCodeTailBytes;
  }
  for (auto& entry : g_jit_method_entries) {
    uintptr_t current = entry.start.load(std::memory_order_acquire);
    if (current == code || current == 0) {
      if (current == 0) {
        uintptr_t expected = 0;
        if (!entry.start.compare_exchange_strong(expected, kJitEntryPublishing,
                                                 std::memory_order_acq_rel,
                                                 std::memory_order_acquire)) {
          continue;
        }
        entry.end.store(end, std::memory_order_relaxed);
        entry.method.store(method, std::memory_order_relaxed);
        entry.start.store(code, std::memory_order_release);
      } else {
        entry.end.store(end, std::memory_order_relaxed);
        entry.method.store(method, std::memory_order_release);
      }
      if (std::getenv("DARWIN_ART_DEBUG_JIT") != nullptr) {
        std::fprintf(stderr, "DARWIN JIT publish code=%p end=%p size=%zu method=%p\n",
                     reinterpret_cast<void*>(code), reinterpret_cast<void*>(end), size,
                     reinterpret_cast<void*>(method));
      }
      return;
    }
  }
}

uintptr_t DarwinArtLookupJitMethod(uintptr_t pc) {
  if (pc == 0) return 0;
  for (const auto& entry : g_jit_method_entries) {
    const uintptr_t start = entry.start.load(std::memory_order_acquire);
    const uintptr_t end = entry.end.load(std::memory_order_acquire);
    if (start > kJitEntryPublishing && pc >= start && pc < end) {
      return entry.method.load(std::memory_order_acquire);
    }
  }
  return 0;
}

extern "C" void DarwinArtRegisterJitCodeRange(uintptr_t code, size_t size) {
  DarwinArtRegisterJitMethod(code, size, 0);
}

extern "C" bool DarwinArtLookupJitCode(uintptr_t pc) {
  if (pc == 0) return false;
  for (const auto& entry : g_jit_method_entries) {
    const uintptr_t start = entry.start.load(std::memory_order_acquire);
    const uintptr_t end = entry.end.load(std::memory_order_acquire);
    if (start > kJitEntryPublishing && pc >= start && pc < end) return true;
  }
  return false;
}

void* DarwinArtMapJitCode(size_t size) {
  if (size == 0 || !pthread_jit_write_protect_supported_np()) {
    errno = ENOTSUP;
    return MAP_FAILED;
  }
  return mmap(nullptr, size, PROT_READ | PROT_WRITE | PROT_EXEC,
              MAP_PRIVATE | MAP_ANON | MAP_JIT, -1, 0);
}

void DarwinArtJitWriteBegin() {
  assert(pthread_jit_write_protect_supported_np());
  if (write_depth++ == 0) pthread_jit_write_protect_np(0);
}

void DarwinArtJitWriteEnd() {
  assert(write_depth > 0);
  if (--write_depth == 0) pthread_jit_write_protect_np(1);
}

unsigned DarwinArtJitWriteDepth() {
  return write_depth;
}

DarwinArtJitWriteScope::DarwinArtJitWriteScope() {
  DarwinArtJitWriteBegin();
}

DarwinArtJitWriteScope::~DarwinArtJitWriteScope() {
  DarwinArtJitWriteEnd();
}

void DarwinArtFlushJitCode(void* code, size_t size) {
  sys_dcache_flush(code, size);
  sys_icache_invalidate(code, size);
}
