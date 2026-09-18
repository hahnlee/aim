#pragma once
#include <jni.h>

namespace darwin_art::media {

// Small framework media JNI contracts kept with the media subsystem.
bool RegisterMediaDrmNatives(JNIEnv* env);
bool RegisterPublicFormatNatives(JNIEnv* env);

}  // namespace darwin_art::media

