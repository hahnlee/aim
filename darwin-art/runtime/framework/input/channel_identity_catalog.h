#pragma once

#include <jni.h>
#include <memory>

namespace darwin_art::input {
class InputChannelResources;
enum class ChannelIdentityStatus { kResolved, kNotFound, kClosed, kJniFailure, kConflict };
struct ChannelIdentityResult {
  ChannelIdentityStatus status = ChannelIdentityStatus::kNotFound;
  std::shared_ptr<InputChannelResources> core;
};

// Java/Binder identity owner, separate from both JNI wrappers and resource core.
// Holds only weak Java tokens and weak cores. Operations pin identity snapshots
// before JNI calls outside the catalog lock.
class InputChannelIdentityCatalog final {
 public:
  InputChannelIdentityCatalog();
  ~InputChannelIdentityCatalog();
  ChannelIdentityResult Bind(
      JNIEnv*, jobject token, std::shared_ptr<InputChannelResources>);
  ChannelIdentityResult Find(JNIEnv*, jobject token);
  // Closes admission. False means an admitted operation still needs the VM;
  // callers must not destroy it. Retry after quiescence deletes all weak refs.
  bool CloseAdmission();
  bool Clear(JNIEnv*);
  InputChannelIdentityCatalog(const InputChannelIdentityCatalog&) = delete;
  InputChannelIdentityCatalog& operator=(const InputChannelIdentityCatalog&) = delete;
 private:
  struct Control;
  std::shared_ptr<Control> control_;
};
InputChannelIdentityCatalog& GetChannelIdentityCatalog();
}  // namespace darwin_art::input
