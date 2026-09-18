#pragma once

#include <jni.h>
#include <memory>

namespace darwin_art::binder { class EndpointLifetime; }

namespace darwin_art::framework::wm {

enum class RootKeyServerStatus { kCaptured, kInvalidIdentity, kUnavailable, kJniFailure };
struct RootKeyServerCapture {
  RootKeyServerStatus status = RootKeyServerStatus::kInvalidIdentity;
  std::shared_ptr<binder::EndpointLifetime> lifetime;
};

// Borrow the registered canonical RemoteBinder class, decode outside wire
// locks, then retain only the proxy's exact still-established generation.
// This is transport lifetime identity, not a focus grant or Binder-node death.
// Neither a numeric-FD fallback nor connection creation is permitted here.
RootKeyServerCapture CaptureRootKeyServer(
    JNIEnv* env, jobject capability, jclass trusted_remote_class);

}  // namespace darwin_art::framework::wm
