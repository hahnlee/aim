#pragma once

#include <jni.h>

namespace aim::media_fixture {

// Fixture-only Java lifecycle check. Production media codec ownership remains
// in compat/darwin_media_codec.cc; this function performs only public Java
// MediaCodec/Surface operations and provider-visible window retention checks.
bool VerifyMediaCodecSurfaceLifecycle(JNIEnv* env);

}  // namespace aim::media_fixture
