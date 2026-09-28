#pragma once
#include <jni.h>

namespace aim::media {
// Owns android.media.ImageReader/SurfaceImage JNI state and native image lifetime.
// BufferQueue slot accounting belongs to ImageConsumerQueue, not this registrar.
bool RegisterImageReaderNatives(JNIEnv* env);
}
