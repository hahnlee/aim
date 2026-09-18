#pragma once
#include <jni.h>

namespace darwin_art::media {

// Owns android.media.MediaExtractor JNI state and Java native-context lifetime.
// WebM parsing remains the shared implementation in darwin_media_extractor.h.
bool RegisterMediaExtractorNatives(JNIEnv* env);

}  // namespace darwin_art::media

