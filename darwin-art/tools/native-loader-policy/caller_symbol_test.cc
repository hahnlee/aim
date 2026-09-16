#include "loader/namespace_handles.h"
#include "loader/namespace_loader_abi.h"
#include <cassert>
#include <cstdio>

void TestCallerSymbolAbi(darwin_art::loader::NamespaceHandles& owner) {
  std::string error;
  const int sdk = owner.TargetSdkVersion();
  assert(owner.SetTargetSdkVersion(36));
  auto* child = owner.CreateChild(owner.Anonymous(), true, false, false,
      nullptr, "/system/lib64", "/system/lib64", &error, "caller-symbol-test");
  assert(child && error.empty());
  assert(owner.Link(child, owner.Anonymous(), "libc.so:libm.so:libdl.so:libc++.so", &error));
  const auto handle = owner.OpenLibrary(0, "libsync.so", 2, ANDROID_DLEXT_USE_NAMESPACE, child, &error);
  assert(handle && error.empty());
  const auto entry = owner.LibrarySymbol(handle, "sync_wait", &error);
  assert(entry && error.empty());
  uintptr_t linear = 99;
  assert(owner.LinearSymbol(child, nullptr, "sync_wait", &linear, &error) == 1);
  assert(linear == 0); // Modern LOCAL image requires caller-local-group fallback.
  const auto* caller = reinterpret_cast<void*>(entry);
  auto* found = __loader_dlsym(nullptr, "sync_wait", caller);
  assert(reinterpret_cast<uintptr_t>(found) == entry);
  assert(reinterpret_cast<int (*)(int, int)>(found)(-1, 0) == -1);
  assert(__loader_dlsym(reinterpret_cast<void*>(handle), "sync_wait", nullptr) == found);
  // Android accepts any requested version when this DSO has no DT_VERSYM.
  assert(__loader_dlvsym(reinterpret_cast<void*>(handle), "sync_wait",
      "UNDECLARED_SYNC_VERSION", nullptr) == found);
  assert(__loader_dlvsym(nullptr, "sync_wait", "UNDECLARED_SYNC_VERSION", caller) == found);
  void* next = reinterpret_cast<void*>(UINTPTR_MAX);
  const auto malloc_symbol = owner.LibrarySymbol(handle, "malloc", &error);
  assert(malloc_symbol && error.empty());
  assert(reinterpret_cast<uintptr_t>(__loader_dlsym(next, "malloc", caller)) == malloc_symbol);
  assert(reinterpret_cast<uintptr_t>(__loader_dlvsym(next, "malloc", "LIBC", caller)) == malloc_symbol);
  assert(reinterpret_cast<uintptr_t>(__loader_dlvsym(nullptr, "malloc", "LIBC", caller)) == malloc_symbol);
  assert(reinterpret_cast<uintptr_t>(__loader_dlvsym(reinterpret_cast<void*>(handle), "malloc", "LIBC", caller)) == malloc_symbol);
  assert(!__loader_dlvsym(next, "malloc", "LIBC_NONEXISTENT", caller));
  assert(darwin_art_linker_dlerror());
  assert(!__loader_dlsym(next, "sync_wait", caller));
  assert(darwin_art_linker_dlerror());
  assert(!__loader_dlsym(next, "malloc", nullptr));
  assert(darwin_art_linker_dlerror());
  assert(__loader_dlclose(reinterpret_cast<void*>(handle)) == 0);
  assert(__loader_dlclose(reinterpret_cast<void*>(handle)) == -1);
  assert(__loader_dlerror());
  assert(!__loader_dlerror()); // Consumes pending state once, like Bionic.
  assert(!darwin_art_linker_dlerror()); // Same thread-owned store, not a copy.
  assert(owner.SetTargetSdkVersion(sdk));
  std::puts("Caller dlsym ABI: DEFAULT local-group fallback, NEXT dependencies, ordinary handle and ELF execution PASS");
}
