#include "typed_library.h"
#include <jni.h>
#include <cstdlib>
#include <cstring>
#include <utility>

namespace darwin_art::jni {
namespace {
bool Fail(std::string* error, const char* message) {
  if (error) *error = message;
  return false;
}
bool HasKind(const LinkerImageLease* image, uint32_t kind) {
  void* payload = nullptr;
  return darwin_art_linker_image_typed_payload(image, kind, &payload) == 0 && payload;
}
}

std::unique_ptr<TypedLibrary> TypedLibrary::Adopt(
    std::shared_ptr<loader::NamespaceHandles> namespaces, uintptr_t handle,
    std::shared_ptr<ProxyVm> vm,
    std::string* error) try {
  if (error) error->clear();
  if (!namespaces || !handle || !vm || !vm->JavaVm()) {
    Fail(error, "JNI library requires an owned namespace open and shared VM");
    return nullptr;
  }
  auto owner = std::unique_ptr<TypedLibrary>(new TypedLibrary());
  LinkerImageLease* image = nullptr;
  if (namespaces->LibraryImage(handle, &image) != 0 || !image) {
    darwin_art_linker_image_release(image);
    Fail(error, "invalid typed JNI library handle");
    return nullptr;
  }
  owner->root_.reset(image);
  if (!HasKind(image, DARWIN_ART_IMAGE_ELF_SELECTED) && !HasKind(image, DARWIN_ART_IMAGE_MACHO)) {
    Fail(error, "JNI library image has no declared callable ABI");
    return nullptr;
  }
  owner->vm_ = std::move(vm);
  owner->namespaces_ = std::move(namespaces);
  owner->handle_ = handle;
  owner->state_ = State::Open;
  return owner;
} catch (...) {
  Fail(error, "cannot allocate typed JNI library owner");
  return nullptr;
}

TypedLibrary::~TypedLibrary() {
  if (state_ != State::Closed) std::abort();
}

void* TypedLibrary::Resolve(const char* name, const char* shorty, CallKind kind,
                           std::string* error) try {
  if (error) error->clear();
  std::lock_guard lock(mutex_);
  auto fail = [&](const char* message) -> void* { Fail(error, message); return nullptr; };
  if (state_ != State::Open) return fail("JNI library is closing or closed");
  if (!name || !*name) return fail("missing JNI symbol name");
  const char* signature = shorty;
  switch (kind) {
    case CallKind::OnLoad:
      if (std::strcmp(name, "JNI_OnLoad") != 0) return fail("invalid JNI load symbol");
      signature = "ILL";
      break;
    case CallKind::OnUnload:
      if (std::strcmp(name, "JNI_OnUnload") != 0) return fail("invalid JNI unload symbol");
      signature = "VLL";
      break;
    case CallKind::Regular:
    case CallKind::Critical:
      if (!signature || !*signature) return fail("missing JNI method shorty");
      break;
    default: return fail("unknown JNI call kind");
  }
  const size_t length = strnlen(signature, 257);
  if (!length || length > 256) return fail("invalid JNI shorty length");
  for (size_t i = 0; i < length; ++i) {
    if ((signature[i] == 'V' && i == 0) ||
        std::strchr(kind == CallKind::Critical ? "ZBCSIJFD" : "ZBCSIJFDL", signature[i])) continue;
    return fail("invalid JNI call-shape shorty");
  }
  const std::string key = std::to_string(static_cast<int>(kind)) + ':' + name + ':' + signature;
  if (auto found = callables_.find(key); found != callables_.end()) return found->second.entry;
  LinkerImageLease* source = nullptr;
  const uintptr_t address = namespaces_->LibrarySymbol(handle_, name, error, nullptr, &source);
  Callable callable;
  callable.image.reset(source);
  if (!address || !source) return nullptr;
  if (HasKind(source, DARWIN_ART_IMAGE_MACHO)) {
    callable.entry = reinterpret_cast<void*>(address);
  } else if (HasKind(source, DARWIN_ART_IMAGE_ELF_SELECTED)) {
    android_jni::TrampolineRequest request{reinterpret_cast<void*>(address), signature, 1};
    void* vm = vm_->JavaVm();
    if (kind == CallKind::Critical) {
      callable.code.reset(android_jni::CreateCriticalTrampolines(&request, 1, error));
    } else if (kind == CallKind::OnLoad || kind == CallKind::OnUnload) {
      callable.code.reset(android_jni::CreateLifecycleTrampolines(vm, &request, 1, error));
    } else {
      void* env = nullptr;
      if (!vm || static_cast<JavaVM*>(vm)->GetEnv(&env, JNI_VERSION_1_6) != JNI_OK || !env)
        return fail("JNI method lookup requires an attached proxy environment");
      callable.code.reset(android_jni::CreateRegularTrampolines(env, &request, 1, error));
    }
    callable.entry = android_jni::TrampolineEntry(callable.code.get(), 0);
    if (!callable.entry) return nullptr;
  } else {
    return fail("defining JNI image has no declared callable ABI");
  }
  void* entry = callable.entry;
  callables_.emplace(key, std::move(callable));
  return entry;
} catch (...) {
  Fail(error, "cannot retain typed JNI callable");
  return nullptr;
}

bool TypedLibrary::CloseAfterQuiescence(std::string* error) {
  if (error) error->clear();
  {
    std::lock_guard lock(mutex_);
    if (state_ == State::Closed) return true;
    if (state_ == State::Closing) return Fail(error, "JNI close is already in progress");
    state_ = State::Closing;
  }
  // Foreign DSO destructors may call through the still-live proxy. Do not
  // hold the callable mutex while the namespace owner runs finalization.
  if (namespaces_->CloseLibrary(handle_, error) != 0) {
    std::lock_guard lock(mutex_);
    state_ = State::Open;
    return false;
  }
  // Release mappings before the context, and never invoke external resource
  // deleters while holding this owner's mutex.
  std::shared_ptr<ProxyVm> retired_vm;
  loader::ImageLease retired_root;
  decltype(callables_) retired_callables;
  {
    std::lock_guard lock(mutex_);
    handle_ = 0;
    retired_callables.swap(callables_);
    retired_root = std::move(root_);
    retired_vm = std::move(vm_);
    state_ = State::Closed;
  }
  return true;
}
}
