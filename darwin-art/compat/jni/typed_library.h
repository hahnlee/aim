#pragma once

#include "loader/namespace_handles.h"
#include "loader/namespace_group_release.h"
#include "darwin_android_jni_trampoline.h"
#include "proxy_vm.h"
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>

namespace darwin_art::jni {

enum class CallKind { Regular, Critical, OnLoad, OnUnload };

// Darwin callable/VM adaptation for one Android NativeLoader open. This does
// not select namespaces, invoke lifecycle methods or own ClassLoader policy.
// ART must invoke OnUnload and establish method-call quiescence before close.
class TypedLibrary final {
 public:
  // Transfer this one logical open only on success. ART supplies its shared VM
  // identity, whose lifetime is separate from any individual native library.
  // No callbacks or JNI lifecycle functions execute during adoption.
  static std::unique_ptr<TypedLibrary> Adopt(
      std::shared_ptr<loader::NamespaceHandles> namespaces, uintptr_t handle,
      std::shared_ptr<ProxyVm> vm,
      std::string* error);
  ~TypedLibrary();
  TypedLibrary(const TypedLibrary&) = delete;
  TypedLibrary& operator=(const TypedLibrary&) = delete;

  // A missing typed symbol never retries through raw host dlsym. Regular and
  // critical calls require the ART shorty; lifecycle kinds derive their ABI.
  void* Resolve(const char* name, const char* shorty, CallKind kind, std::string* error);
  // Caller explicitly guarantees no remaining/future calls through this owner's
  // generated entries. The shared VM remains alive under ART's separate root.
  // CloseLibrary success alone is NOT that guarantee. Failure retains the
  // complete owner for retry. Destruction before successful explicit close is
  // fatal rather than silently assuming quiescence or freeing live code.
  bool CloseAfterQuiescence(std::string* error);

 private:
  TypedLibrary() = default;
  struct Callable {
    loader::ImageLease image;
    std::unique_ptr<android_jni::TrampolineSet,
                    decltype(&android_jni::DestroyRegularTrampolines)> code{
        nullptr, android_jni::DestroyRegularTrampolines};
    void* entry = nullptr;
  };
  enum class State { Open, Closing, Closed };
  std::recursive_mutex mutex_;
  State state_ = State::Closed;
  std::shared_ptr<loader::NamespaceHandles> namespaces_;
  std::shared_ptr<ProxyVm> vm_;
  loader::ImageLease root_;
  uintptr_t handle_ = 0;
  std::unordered_map<std::string, Callable> callables_;
};
}
