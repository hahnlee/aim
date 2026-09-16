#pragma once

#include "darwin_art_jni_proxy.h"
#include <jni.h>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>

namespace darwin_art::jni {

// ART's registration boundary must resolve method flags and retain/adapt the
// defining native image before installation. Raw Android function pointers must
// never be handed directly to the host RegisterNatives table.
class NativeRegistration {
public:
  virtual ~NativeRegistration() = default;
  // Convert failures to JNI status/exceptions; C++ exceptions must not cross
  // the guest C callback boundary.
  virtual jint Register(JNIEnv *, jclass, const DarwinArtJniNativeMethod *,
                        jint) noexcept = 0;
};

// Owned callback/descriptor state for a guest JNI proxy. ART owns JavaVM and
// must keep it alive through this context's final callback and destruction.
// No ClassLoader references or namespace/lifecycle policy are retained here.
// Attachment is explicit: GetEnv/lookup do not silently attach guest workers.
class VmContext final {
public:
  static std::shared_ptr<VmContext> Create(JavaVM *,
                                           std::shared_ptr<NativeRegistration>);
  // The returned C table borrows this context; the proxy owner must retain the
  // shared context until its VM/env pointers and all native calls are
  // quiescent.
  DarwinArtJniBackend Backend();

private:
  VmContext(JavaVM *vm, std::shared_ptr<NativeRegistration> registration);
  JNIEnv *Environment() const;
  static void *CurrentEnvironment(void *) noexcept;
  static int32_t Attach(void *, void *, int32_t) noexcept;
  static int32_t Detach(void *) noexcept;
  static void *FindClass(void *, const char *) noexcept;
  static int32_t Register(void *, void *, const DarwinArtJniNativeMethod *,
                          int32_t) noexcept;
  static int32_t ThrowNew(void *, void *, const char *) noexcept;
  static void *GetMethod(void *, void *, const char *, const char *, int32_t) noexcept;
  static uint64_t CallMethod(void *, void *, void *, void *, int32_t, int32_t) noexcept;

  JavaVM *const vm_;
  const std::shared_ptr<NativeRegistration> registration_;
  std::mutex methods_mutex_;
  std::unordered_map<jmethodID, std::string> descriptors_;
};
} // namespace darwin_art::jni
