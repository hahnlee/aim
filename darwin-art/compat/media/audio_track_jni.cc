#include "audio_track_jni.h"

#include "../darwin_audio_track.h"
#include "audio_system_jni.h"

#include <algorithm>
#include <cstdint>
#include <iterator>
#include <vector>

namespace {

jfieldID AudioTrackNativeField(JNIEnv* env, jobject track) {
  jclass clazz = env->GetObjectClass(track);
  if (clazz == nullptr) return nullptr;
  jfieldID field = env->GetFieldID(clazz, "mNativeTrackInJavaObj", "J");
  env->DeleteLocalRef(clazz);
  return field;
}

DarwinAudioTrack* GetAudioTrack(JNIEnv* env, jobject track) {
  jfieldID field = AudioTrackNativeField(env, track);
  if (field == nullptr) return nullptr;
  return reinterpret_cast<DarwinAudioTrack*>(
      static_cast<uintptr_t>(env->GetLongField(track, field)));
}

jint AudioTrackGetOutputSampleRate(JNIEnv*, jclass, jint) { return 48000; }
jint AudioTrackGetMinBufferSize(JNIEnv*, jclass, jint sample_rate,
                                jint channel_count, jint audio_format) {
  if (sample_rate <= 0 || channel_count <= 0 || audio_format <= 0) return -2;
  // Twenty milliseconds of stereo PCM16, rounded up to a practical queue
  // quantum. AudioTrack.native_setup owns the actual Darwin output stream.
  const jint bytes = (sample_rate / 50) * channel_count * 2;
  return std::max<jint>(4096, bytes);
}
jint AudioTrackSetup(JNIEnv* env, jobject track, jobject, jobject,
                     jintArray sample_rates, jint channel_mask, jint,
                     jint audio_format, jint buffer_bytes, jint,
                     jintArray session_ids, jobject, jlong, jboolean, jint,
                     jobject, jstring) {
  jint sample_rate = 48000;
  if (sample_rates != nullptr && env->GetArrayLength(sample_rates) > 0) {
    env->GetIntArrayRegion(sample_rates, 0, 1, &sample_rate);
  }
  buffer_bytes = std::max<jint>(4096, buffer_bytes);
  if (session_ids != nullptr && env->GetArrayLength(session_ids) > 0) {
    jint session = 0;
    env->GetIntArrayRegion(session_ids, 0, 1, &session);
    if (session == 0) {
      session = darwin_art::media::AllocateAudioSessionId();
      env->SetIntArrayRegion(session_ids, 0, 1, &session);
    }
  }
  jfieldID field = AudioTrackNativeField(env, track);
  if (field == nullptr) return -20;
  DarwinAudioTrack* state = darwin_audio_track_create(
      sample_rate, channel_mask, audio_format, buffer_bytes);
  if (state == nullptr) return -20;
  env->SetLongField(track, field,
                    static_cast<jlong>(reinterpret_cast<uintptr_t>(state)));
  return 0;
}
void AudioTrackRelease(JNIEnv* env, jobject track) {
  jfieldID field = AudioTrackNativeField(env, track);
  if (field == nullptr) return;
  auto* state = reinterpret_cast<DarwinAudioTrack*>(
      static_cast<uintptr_t>(env->GetLongField(track, field)));
  env->SetLongField(track, field, 0);
  darwin_audio_track_destroy(state);
}
void AudioTrackVoid(JNIEnv*, jobject) {}
void AudioTrackStart(JNIEnv* env, jobject track) {
  darwin_audio_track_start(GetAudioTrack(env, track));
}
void AudioTrackStop(JNIEnv* env, jobject track) {
  darwin_audio_track_stop(GetAudioTrack(env, track));
}
void AudioTrackPause(JNIEnv* env, jobject track) {
  darwin_audio_track_pause(GetAudioTrack(env, track));
}
void AudioTrackFlush(JNIEnv* env, jobject track) {
  darwin_audio_track_flush(GetAudioTrack(env, track));
}
void AudioTrackSetPlayerId(JNIEnv*, jobject, jint) {}
void AudioTrackSetVolume(JNIEnv* env, jobject track, jfloat left, jfloat right) {
  darwin_audio_track_set_volume(GetAudioTrack(env, track), left, right);
}
jint AudioTrackStatusInt(JNIEnv*, jobject, jint) { return 0; }
jint AudioTrackGetInt(JNIEnv* env, jobject track) {
  return darwin_audio_track_buffer_capacity_frames(GetAudioTrack(env, track));
}
jint AudioTrackGetPosition(JNIEnv* env, jobject track) {
  return static_cast<jint>(darwin_audio_track_position(GetAudioTrack(env, track)));
}
jint AudioTrackWriteByte(JNIEnv* env, jobject track, jbyteArray array,
                         jint offset, jint size, jint, jboolean blocking) {
  DarwinAudioTrack* state = GetAudioTrack(env, track);
  if (state == nullptr) return -3;
  if (array == nullptr || offset < 0 || size < 0 ||
      offset > env->GetArrayLength(array) - size) return -2;
  std::vector<jbyte> bytes(static_cast<size_t>(size));
  if (size != 0) env->GetByteArrayRegion(array, offset, size, bytes.data());
  if (env->ExceptionCheck()) return -3;
  return static_cast<jint>(darwin_audio_track_write(
      state, bytes.data(), bytes.size(), blocking == JNI_TRUE));
}
jint AudioTrackWriteShort(JNIEnv* env, jobject track, jshortArray array,
                          jint offset, jint size, jint, jboolean blocking) {
  DarwinAudioTrack* state = GetAudioTrack(env, track);
  if (state == nullptr) return -3;
  if (array == nullptr || offset < 0 || size < 0 ||
      offset > env->GetArrayLength(array) - size) return -2;
  std::vector<jshort> samples(static_cast<size_t>(size));
  if (size != 0) env->GetShortArrayRegion(array, offset, size, samples.data());
  if (env->ExceptionCheck()) return -3;
  const size_t bytes = darwin_audio_track_write(
      state, samples.data(), samples.size() * sizeof(jshort),
      blocking == JNI_TRUE);
  return static_cast<jint>(bytes / sizeof(jshort));
}
jint AudioTrackWriteFloat(JNIEnv* env, jobject track, jfloatArray array,
                          jint offset, jint size, jint, jboolean blocking) {
  DarwinAudioTrack* state = GetAudioTrack(env, track);
  if (state == nullptr) return -3;
  if (array == nullptr || offset < 0 || size < 0 ||
      offset > env->GetArrayLength(array) - size) return -2;
  std::vector<jfloat> samples(static_cast<size_t>(size));
  if (size != 0) env->GetFloatArrayRegion(array, offset, size, samples.data());
  if (env->ExceptionCheck()) return -3;
  const size_t bytes = darwin_audio_track_write(
      state, samples.data(), samples.size() * sizeof(jfloat),
      blocking == JNI_TRUE);
  return static_cast<jint>(bytes / sizeof(jfloat));
}
jint AudioTrackWriteBuffer(JNIEnv* env, jobject track, jobject buffer,
                           jint offset, jint size, jint, jboolean blocking) {
  DarwinAudioTrack* state = GetAudioTrack(env, track);
  if (state == nullptr) return -3;
  auto* data = static_cast<uint8_t*>(env->GetDirectBufferAddress(buffer));
  const jlong capacity = env->GetDirectBufferCapacity(buffer);
  if (data == nullptr || offset < 0 || size < 0 || capacity < 0 ||
      static_cast<jlong>(offset) > capacity - size) return -2;
  return static_cast<jint>(darwin_audio_track_write(
      state, data + offset, static_cast<size_t>(size), blocking == JNI_TRUE));
}

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

bool RegisterAudioTrackNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("native_get_output_sample_rate"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&AudioTrackGetOutputSampleRate)},
      {const_cast<char*>("native_get_min_buff_size"), const_cast<char*>("(III)I"),
       reinterpret_cast<void*>(&AudioTrackGetMinBufferSize)},
      {const_cast<char*>("native_setup"), const_cast<char*>("(Ljava/lang/Object;Ljava/lang/Object;[IIIIII[ILandroid/os/Parcel;JZILjava/lang/Object;Ljava/lang/String;)I"),
       reinterpret_cast<void*>(&AudioTrackSetup)},
      {const_cast<char*>("native_start"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackStart)},
      {const_cast<char*>("native_stop"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackStop)},
      {const_cast<char*>("native_pause"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackPause)},
      {const_cast<char*>("native_flush"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackFlush)},
      {const_cast<char*>("native_enableDeviceCallback"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackVoid)},
      {const_cast<char*>("native_disableDeviceCallback"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackVoid)},
      {const_cast<char*>("native_release"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackRelease)},
      {const_cast<char*>("native_finalize"), const_cast<char*>("()V"), reinterpret_cast<void*>(&AudioTrackRelease)},
      {const_cast<char*>("native_setPlayerIId"), const_cast<char*>("(I)V"), reinterpret_cast<void*>(&AudioTrackSetPlayerId)},
      {const_cast<char*>("native_setVolume"), const_cast<char*>("(FF)V"), reinterpret_cast<void*>(&AudioTrackSetVolume)},
      {const_cast<char*>("native_set_playback_rate"), const_cast<char*>("(I)I"), reinterpret_cast<void*>(&AudioTrackStatusInt)},
      {const_cast<char*>("native_get_buffer_capacity_frames"), const_cast<char*>("()I"), reinterpret_cast<void*>(&AudioTrackGetInt)},
      {const_cast<char*>("native_get_buffer_size_frames"), const_cast<char*>("()I"), reinterpret_cast<void*>(&AudioTrackGetInt)},
      {const_cast<char*>("native_get_position"), const_cast<char*>("()I"), reinterpret_cast<void*>(&AudioTrackGetPosition)},
      {const_cast<char*>("native_write_byte"), const_cast<char*>("([BIIIZ)I"), reinterpret_cast<void*>(&AudioTrackWriteByte)},
      {const_cast<char*>("native_write_short"), const_cast<char*>("([SIIIZ)I"), reinterpret_cast<void*>(&AudioTrackWriteShort)},
      {const_cast<char*>("native_write_float"), const_cast<char*>("([FIIIZ)I"), reinterpret_cast<void*>(&AudioTrackWriteFloat)},
      {const_cast<char*>("native_write_native_bytes"), const_cast<char*>("(Ljava/nio/ByteBuffer;IIIZ)I"), reinterpret_cast<void*>(&AudioTrackWriteBuffer)},
  };
  return Register(env, "android/media/AudioTrack", methods,
                  static_cast<jint>(std::size(methods)));
}

}  // namespace darwin_art::media
