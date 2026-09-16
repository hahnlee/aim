#pragma once
#include "darwin_art_jni_proxy.h"
#include <array>
#include <memory>
#include <string>

namespace darwin_art::jni {
// ART creates one proxy identity for its VM and retains the root reference
// until VM shutdown. Libraries only borrow shared ownership of this identity.
// All native users (including cached JavaVM/JNIEnv pointers) must be quiescent
// before ART releases that root. Image dlclose is NOT VM shutdown.
class ProxyVm final {
 public:
  static std::shared_ptr<ProxyVm> Create(std::shared_ptr<void> context,
      const DarwinArtJniBackend& backend, std::string* error);
  void* JavaVm() const;
  ProxyVm(const ProxyVm&) = delete;
  ProxyVm& operator=(const ProxyVm&) = delete;
 private:
  ProxyVm() = default;
  std::shared_ptr<void> context_;
  alignas(DARWIN_ART_JNI_PROXY_STORAGE_ALIGNMENT)
      std::array<unsigned char, DARWIN_ART_JNI_PROXY_STORAGE_SIZE> storage_{};
  DarwinArtJniProxy* proxy_ = nullptr;
};
}
