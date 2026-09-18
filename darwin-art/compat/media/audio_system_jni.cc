#include "audio_system_jni.h"

#include "../darwin_audio_track.h"

#include <atomic>
#include <cstdint>
#include <iterator>

namespace {

std::atomic<jint> g_audio_session_id{1};

jint AudioSystemGetMaxChannelCount(JNIEnv*, jclass) { return 24; }
jint AudioSystemGetMaxSampleRate(JNIEnv*, jclass) { return 192000; }
jint AudioSystemGetMinSampleRate(JNIEnv*, jclass) { return 4000; }
jint AudioSystemGetPrimaryOutputSampleRate(JNIEnv*, jclass) {
  return darwin_audio_primary_output_sample_rate();
}
jint AudioSystemGetPrimaryOutputFrameCount(JNIEnv*, jclass) {
  return darwin_audio_primary_output_frame_count();
}
jint AudioSystemNewAudioSessionId(JNIEnv*, jclass) {
  return darwin_art::media::AllocateAudioSessionId();
}
jint AudioSystemListPorts(JNIEnv* env, jclass, jobject, jintArray generation) {
  if (generation != nullptr && env->GetArrayLength(generation) > 0) {
    const jint value = 1;
    env->SetIntArrayRegion(generation, 0, 1, &value);
  }
  // AUDIO_STATUS_OK with an empty list is the valid detached-host topology;
  // AudioTrack itself is backed by the Darwin audio implementation.
  return 0;
}
jint AudioSystemSetParameters(JNIEnv*, jclass, jstring) { return 0; }
jint AudioProductStrategyList(JNIEnv*, jclass, jobject) { return 0; }
void AudioPortEventNativeSetup(JNIEnv*, jobject, jobject) {}
void AudioPortEventNativeFinalize(JNIEnv*, jobject) {}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) return false;
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

}  // namespace

namespace darwin_art::media {

jint AllocateAudioSessionId() {
  return g_audio_session_id.fetch_add(1, std::memory_order_relaxed);
}

bool RegisterAudioSystemNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("native_getMaxChannelCount"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&AudioSystemGetMaxChannelCount)},
      {const_cast<char*>("native_getMaxSampleRate"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&AudioSystemGetMaxSampleRate)},
      {const_cast<char*>("native_getMinSampleRate"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&AudioSystemGetMinSampleRate)},
      {const_cast<char*>("getPrimaryOutputSamplingRate"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&AudioSystemGetPrimaryOutputSampleRate)},
      {const_cast<char*>("getPrimaryOutputFrameCount"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&AudioSystemGetPrimaryOutputFrameCount)},
      {const_cast<char*>("newAudioSessionId"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&AudioSystemNewAudioSessionId)},
      {const_cast<char*>("listAudioPorts"), const_cast<char*>("(Ljava/util/ArrayList;[I)I"),
       reinterpret_cast<void*>(&AudioSystemListPorts)},
      {const_cast<char*>("listAudioPatches"), const_cast<char*>("(Ljava/util/ArrayList;[I)I"),
       reinterpret_cast<void*>(&AudioSystemListPorts)},
      {const_cast<char*>("setParameters"), const_cast<char*>("(Ljava/lang/String;)I"),
       reinterpret_cast<void*>(&AudioSystemSetParameters)},
  };
  return Register(env, "android/media/AudioSystem", methods,
                  static_cast<jint>(std::size(methods)));
}

bool RegisterAudioProductStrategyNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("native_list_audio_product_strategies"),
       const_cast<char*>("(Ljava/util/ArrayList;)I"),
       reinterpret_cast<void*>(&AudioProductStrategyList)},
  };
  return Register(env, "android/media/audiopolicy/AudioProductStrategy", methods,
                  static_cast<jint>(std::size(methods)));
}

bool RegisterAudioPortEventHandlerNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("native_setup"), const_cast<char*>("(Ljava/lang/Object;)V"),
       reinterpret_cast<void*>(&AudioPortEventNativeSetup)},
      {const_cast<char*>("native_finalize"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&AudioPortEventNativeFinalize)},
  };
  return Register(env, "android/media/AudioPortEventHandler", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::media
