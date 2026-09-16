#pragma once
#include "registered_methods.h"
#include <jni.h>

namespace darwin_art::jni {
// One ART JavaVM's native ABI resources, constructed only after its shared
// process namespace exists. The ART registration hook must route to Resolve
// before this owner's proxy is exposed to native code. Creation publishes
// nothing globally and never initializes a second JavaVM or namespace.
class RuntimeVm final {
 public:
  static std::unique_ptr<RuntimeVm> Create(
      JavaVM*, std::shared_ptr<loader::NamespaceHandles>, std::string* error);
  const std::shared_ptr<ProxyVm>& proxy() const { return proxy_; }
  JavaVM* art_vm() const { return art_vm_; }
  DarwinArtRegisteredNativeResolution Resolve(const void* function, bool bridged,
      const char* shorty, uint32_t length, DarwinArtJniCallType kind, const void** out);
  void RetireGroup(uint64_t group);
  // ART must drain native users/libraries before destruction. ART JavaVM must
  // outlive this owner and every shared proxy reference handed to a library.
  ~RuntimeVm();
  RuntimeVm(const RuntimeVm&) = delete;
  RuntimeVm& operator=(const RuntimeVm&) = delete;
 private:
  RuntimeVm() = default;
  JavaVM* art_vm_ = nullptr; // borrowed from ART, never DeleteJavaVM here
  std::shared_ptr<ProxyVm> proxy_;
  std::unique_ptr<RegisteredMethods> methods_;
};
}
