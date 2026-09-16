#pragma once

#include "proxy_vm.h"
#include "loader/namespace_handles.h"
#include "darwin_art_registered_native_bridge.h"
#include <memory>

namespace darwin_art::jni {
// ART-VM-scoped native callable owner. Namespace policy and method validation
// remain in NativeLoader/ART. This owner only adapts proven ELF code addresses.
class RegisteredMethods final {
 public:
  static std::unique_ptr<RegisteredMethods> Create(
      std::shared_ptr<loader::NamespaceHandles>, std::shared_ptr<ProxyVm>);
  ~RegisteredMethods();
  RegisteredMethods(const RegisteredMethods&) = delete;
  RegisteredMethods& operator=(const RegisteredMethods&) = delete;
  DarwinArtRegisteredNativeResolution Resolve(const void*, bool bridged,
      const char* shorty, uint32_t length, DarwinArtJniCallType, const void**);
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
