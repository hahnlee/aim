#include "remote_binder_identity_jni.h"

#include <bit>

namespace darwin_art::binder {
namespace {

bool HasPendingException(JNIEnv* env) noexcept {
  return env == nullptr || env->ExceptionCheck() == JNI_TRUE;
}

}  // namespace

RemoteBinderIdentity ReadRemoteBinderIdentity(
    JNIEnv* env, jobject binder, jclass trusted_endpoint_class) noexcept {
  RemoteBinderIdentity result;
  if (env == nullptr || binder == nullptr || trusted_endpoint_class == nullptr ||
      HasPendingException(env)) {
    return result;
  }

  // Field names alone are not an identity check: ordinary local Binder
  // implementations may expose fields with the same shape.
  const jboolean is_remote =
      env->IsInstanceOf(binder, trusted_endpoint_class);
  if (HasPendingException(env)) return result;
  if (is_remote == JNI_FALSE) {
    result.kind = RemoteBinderIdentityKind::kLocal;
    return result;
  }

  const jfieldID control_fd =
      env->GetFieldID(trusted_endpoint_class, "controlFd", "I");
  if (HasPendingException(env)) return result;
  if (control_fd == nullptr) return result;
  const jfieldID target_id =
      env->GetFieldID(trusted_endpoint_class, "targetId", "I");
  if (HasPendingException(env)) return result;
  if (target_id == nullptr) return result;
  const jfieldID channel_generation =
      env->GetFieldID(trusted_endpoint_class, "channelGeneration", "J");
  if (HasPendingException(env)) return result;
  if (channel_generation == nullptr) return result;

  const jint fd = env->GetIntField(binder, control_fd);
  if (HasPendingException(env)) return result;
  const jint target = env->GetIntField(binder, target_id);
  if (HasPendingException(env)) return result;
  const jlong generation = env->GetLongField(binder, channel_generation);
  if (HasPendingException(env)) return result;

  if (fd < 0 || target <= 0 || generation == 0) return result;

  result.kind = RemoteBinderIdentityKind::kRemote;
  result.control_fd = fd;
  result.target_id = static_cast<uint32_t>(target);
  // Preserve the Java long's complete two's-complement bit pattern.  In
  // particular, a valid generation may be represented by a negative jlong.
  result.channel_generation = std::bit_cast<uint64_t>(generation);
  return result;
}

}  // namespace darwin_art::binder
