#include "root_key_server_jni.h"

#include <utility>
#include <cstdio>
#include <cstdlib>

#include "../../../compat/binder/remote_binder_identity_jni.h"
#include "../../../compat/binder/wire_channel_lifetime.h"
#include "../../../compat/darwin_binder_wire.h"
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
#include "../../../compat/binder/native_endpoint_lifetime.h"
#endif

namespace darwin_art::framework::wm {

RootKeyServerCapture CaptureRootKeyServer(
    JNIEnv* env, jobject capability, jclass trusted_remote_class) {
  RootKeyServerCapture result;
#if defined(DARWIN_ART_ORIGINAL_BINDER_JNI)
  (void)trusted_remote_class;
  result.lifetime = binder::CaptureNativeBinderEndpoint(env, capability);
  if (env != nullptr && env->ExceptionCheck()) {
    result.status = RootKeyServerStatus::kJniFailure;
  } else {
    result.status = result.lifetime != nullptr ? RootKeyServerStatus::kCaptured
                                             : RootKeyServerStatus::kUnavailable;
  }
  return result;
#else
  const auto identity =
      binder::ReadRemoteBinderIdentity(env, capability, trusted_remote_class);
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    std::fprintf(stderr, "ART RootKey server capture kind=%u fd=%d generation=%llu exception=%u\n",
                 static_cast<unsigned>(identity.kind), identity.control_fd,
                 static_cast<unsigned long long>(identity.channel_generation),
                 static_cast<unsigned>(env != nullptr && env->ExceptionCheck()));
  }
  if (env != nullptr && env->ExceptionCheck()) {
    result.status = RootKeyServerStatus::kJniFailure;
    return result;
  }
  if (identity.kind != binder::RemoteBinderIdentityKind::kRemote) return result;

  // Callback-capable JNI above may retire the channel or reuse its FD. The
  // original generation, never the FD alone, remains the lookup authority.
  auto lifetime = FindRemoteBinderChannelLifetime(
      identity.control_fd, identity.channel_generation);
  if (lifetime == nullptr || !lifetime->Matches(identity.channel_generation)) {
    result.status = RootKeyServerStatus::kUnavailable;
    return result;
  }
  result.lifetime = std::move(lifetime);
  result.status = RootKeyServerStatus::kCaptured;
  return result;
#endif
}

}  // namespace darwin_art::framework::wm
