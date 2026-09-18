#pragma once

#include <jni.h>

namespace darwin_art::media {

// Audio policy/session registration is owned by this media subsystem. The
// session allocator is shared with AudioTrack setup without exposing its
// storage or the host audio provider.
bool RegisterAudioSystemNatives(JNIEnv* env);
bool RegisterAudioProductStrategyNatives(JNIEnv* env);
bool RegisterAudioPortEventHandlerNatives(JNIEnv* env);
jint AllocateAudioSessionId();

}  // namespace darwin_art::media
