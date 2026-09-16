#pragma once

#include "vm_context.h"

namespace darwin_art::jni {

// Thin ART registration owner.  This class deliberately does not adapt raw
// Android function pointers: before any production caller constructs or uses
// it, ART's pointer-adaptation hook must already be installed at the owning
// registration seam.  Construction itself does not install or verify that
// hook, and this header is not wired into production registration by itself.
class ArtRegistration final : public NativeRegistration {
public:
  ArtRegistration() = default;
  ~ArtRegistration() override = default;

  ArtRegistration(const ArtRegistration &) = delete;
  ArtRegistration &operator=(const ArtRegistration &) = delete;

  // Forwards the original method names, descriptors and function pointers to
  // the real JNIEnv::RegisterNatives.  No lookup, descriptor parsing,
  // trampoline rewriting, exception clearing or rollback is performed.
  jint Register(JNIEnv *env, jclass clazz,
                const DarwinArtJniNativeMethod *methods,
                jint count) noexcept override;
};

} // namespace darwin_art::jni
