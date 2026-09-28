#pragma once
#include <stddef.h>
#include <stdint.h>

// Caller owns the mapping; retire all executions before munmap. MAP_FAILED on
// error. Requires the signed host JIT entitlement and thread protection support.
void* AimMapJitCode(size_t size);
void AimFlushJitCode(void* code, size_t size);
void AimJitWriteBegin();
void AimJitWriteEnd();
// Signal-safe diagnostic for the current thread. A non-zero value means
// MAP_JIT is writable and therefore intentionally non-executable here.
unsigned AimJitWriteDepth();

// Signal-safe PC-to-method publication for implicit-null recovery. Entries are
// written by the JIT after code commit and read lock-free from the fault path.
void AimRegisterJitMethod(uintptr_t code, size_t size, uintptr_t method);
uintptr_t AimLookupJitMethod(uintptr_t pc);
extern "C" void AimRegisterJitCodeRange(uintptr_t code, size_t size);
extern "C" bool AimLookupJitCode(uintptr_t pc);

// Thread-affine and nestable. Never execute JIT code inside a write scope.
class AimJitWriteScope {
 public:
  AimJitWriteScope();
  ~AimJitWriteScope();
  AimJitWriteScope(const AimJitWriteScope&) = delete;
  AimJitWriteScope& operator=(const AimJitWriteScope&) = delete;
};
