#pragma once

#include "proxy_vm.h"
#include "loader/namespace_handles.h"
#include "aim_registered_native_bridge.h"
#include <memory>

namespace aim::jni {
// ART-VM-scoped native callable owner. Namespace policy and method validation
// remain in NativeLoader/ART. This owner only adapts proven ELF code addresses.
class RegisteredMethods final {
 public:
  static std::unique_ptr<RegisteredMethods> Create(
      std::shared_ptr<loader::NamespaceHandles>, std::shared_ptr<ProxyVm>);
  ~RegisteredMethods();
  RegisteredMethods(const RegisteredMethods&) = delete;
  RegisteredMethods& operator=(const RegisteredMethods&) = delete;
  AimRegisteredNativeResolution Resolve(const void*, bool bridged,
      const char* shorty, uint32_t length, AimJniCallType, const void**);
  // Caller must stop registration/calls for this original ELF mapping group.
  // Invoke before final unmapping, not on every logical dlclose. Other groups
  // remain callable. Destruction requires global native-call quiescence.
  void RetireGroup(uint64_t group);
 private:
  RegisteredMethods();
  struct State;
  std::unique_ptr<State> state_;
};
}
