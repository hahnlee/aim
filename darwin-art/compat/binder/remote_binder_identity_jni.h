#pragma once

#include <jni.h>

#include <cstdint>

namespace darwin_art::binder {

// This is only the JNI representation of a RemoteBinder endpoint.  The
// caller remains responsible for checking that the endpoint belongs to the
// currently authorized wire owner before using these values.
enum class RemoteBinderIdentityKind {
  kLocal,
  kRemote,
  kInvalid,
};

struct RemoteBinderIdentity {
  RemoteBinderIdentityKind kind = RemoteBinderIdentityKind::kInvalid;
  jint control_fd = -1;
  uint32_t target_id = 0;
  uint64_t channel_generation = 0;
};

// Decode a RemoteBinder only when it is an instance of the caller-supplied
// class.  The class is borrowed: this helper never looks it up or releases
// it, and it never changes a pending JNI exception.
RemoteBinderIdentity ReadRemoteBinderIdentity(
    JNIEnv* env, jobject binder, jclass trusted_endpoint_class) noexcept;

}  // namespace darwin_art::binder
