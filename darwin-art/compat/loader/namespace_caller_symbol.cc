#include "namespace_handles.h"
#include "namespace_operation.h"
#include "namespace_group_release.h"
#include "namespace_symbol_lookup.h"
#include "namespace_loader_abi.h"
#include "process_namespaces.h"

namespace darwin_art::loader {
uintptr_t NamespaceHandles::SymbolForCaller(uintptr_t address, uintptr_t handle,
    const char* symbol, std::string* error, const char* version) {
  if (error) error->clear();
  auto fail = [error](const char* reason) -> uintptr_t { if (error) *error = reason; return 0; };
  if (!symbol || !*symbol) return fail("missing Android symbol name");
  // Android arm64 pseudo-handles, not Darwin's RTLD_DEFAULT/RTLD_NEXT values.
  constexpr uintptr_t next_handle = UINTPTR_MAX;
  if (handle != 0 && handle != next_handle) return LibrarySymbol(handle, symbol, error, version);
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter caller symbol operation");
  LinkerImageLease* raw_caller = nullptr;
  if (FindElfCaller(address, &raw_caller, error) < 0) return 0;
  ImageLease caller(raw_caller);
  const bool next = handle == next_handle;
  if (next && !caller) return fail("RTLD_NEXT requires a registered caller");
  auto* ns = ParentForCaller(nullptr, caller.get(), error);
  if (!ns) return 0;
  uintptr_t result = 0;
  const int linear = LinearSymbol(ns, next ? caller.get() : nullptr, symbol, &result, error, version);
  if (linear < 0) return 0; // Unsupported metadata is not a search miss.
  if (linear == 0) return result;
  if (!caller) return fail("Android symbol not found in anonymous namespace");
  LinkerImageLease* raw_root = nullptr;
  if (darwin_art_linker_image_local_group_root(configured_->registry(), caller.get(), &raw_root) != 0)
    return fail("original caller local-group root unavailable");
  ImageLease root(raw_root);
  return LookupNamespaceSymbolAfter(configured_->registry(), root.get(),
      next ? caller.get() : nullptr, symbol, error, version);
}
}

extern "C" void* __loader_dlvsym(void* handle, const char* symbol, const char* version,
    const void* caller) {
  auto owner = darwin_art::loader::AcquireProcessNamespaces();
  if (!owner) {
    darwin_art_linker_set_error("Android process namespaces are not installed");
    return nullptr;
  }
  std::string error;
  const auto address = owner->SymbolForCaller(reinterpret_cast<uintptr_t>(caller),
      reinterpret_cast<uintptr_t>(handle), symbol, &error, version);
  if (!address) darwin_art_linker_set_error(error.c_str());
  return reinterpret_cast<void*>(address);
}
extern "C" void* __loader_dlsym(void* handle, const char* symbol, const void* caller) {
  return __loader_dlvsym(handle, symbol, nullptr, caller);
}
extern "C" void* darwin_art_linker_dlsym(void* handle, const char* symbol) {
  return __loader_dlsym(handle, symbol, __builtin_return_address(0));
}
