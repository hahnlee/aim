#pragma once
#include <jni.h>

namespace aim::media {

// Small framework media JNI contracts kept with the media subsystem.
bool RegisterMediaDrmNatives(JNIEnv* env);
bool RegisterPublicFormatNatives(JNIEnv* env);

}  // namespace aim::media

