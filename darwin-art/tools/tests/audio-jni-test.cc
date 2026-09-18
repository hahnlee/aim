#include "media/audio_system_jni.h"
#include "media/audio_track_jni.h"

#include <cassert>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <unordered_map>
#include <vector>

struct DarwinAudioTrack {};

namespace {

struct TrackObject : _jobject {
  jlong native = 0;
};

struct Method {
  std::string signature;
  void* function = nullptr;
};

std::unordered_map<std::string, Method> methods;
std::vector<std::string> registration_order;
std::string current_class;
bool fail_registration = false;
DarwinAudioTrack track_state;
int create_calls = 0;
int destroy_calls = 0;

std::string Key(const char* class_name, const char* method_name) {
  return std::string(class_name) + "::" + method_name;
}

jclass FindClass(JNIEnv*, const char* name) {
  current_class = name;
  return reinterpret_cast<jclass>(1);
}

jint RegisterNatives(JNIEnv*, jclass, const JNINativeMethod* entries,
                     jint count) {
  if (fail_registration) return JNI_ERR;
  registration_order.push_back(current_class);
  for (jint i = 0; i < count; ++i) {
    methods.emplace(Key(current_class.c_str(), entries[i].name),
                    Method{entries[i].signature, entries[i].fnPtr});
  }
  return JNI_OK;
}

void DeleteLocalRef(JNIEnv*, jobject) {}
jclass GetObjectClass(JNIEnv*, jobject) { return reinterpret_cast<jclass>(2); }
jfieldID GetFieldID(JNIEnv*, jclass, const char* name, const char* signature) {
  assert(std::strcmp(name, "mNativeTrackInJavaObj") == 0);
  assert(std::strcmp(signature, "J") == 0);
  return reinterpret_cast<jfieldID>(3);
}
jlong GetLongField(JNIEnv*, jobject object, jfieldID) {
  return static_cast<TrackObject*>(object)->native;
}
void SetLongField(JNIEnv*, jobject object, jfieldID, jlong value) {
  static_cast<TrackObject*>(object)->native = value;
}

template <typename Function>
Function Native(const char* class_name, const char* method_name,
                const char* signature) {
  const auto found = methods.find(Key(class_name, method_name));
  assert(found != methods.end());
  assert(found->second.signature == signature);
  return reinterpret_cast<Function>(found->second.function);
}

}  // namespace

extern "C" int32_t darwin_audio_primary_output_sample_rate() { return 48000; }
extern "C" int32_t darwin_audio_primary_output_frame_count() { return 960; }
extern "C" DarwinAudioTrack* darwin_audio_track_create(int32_t, int32_t,
                                                          int32_t, int32_t) {
  ++create_calls;
  return &track_state;
}
extern "C" void darwin_audio_track_destroy(DarwinAudioTrack* track) {
  if (track != nullptr) ++destroy_calls;
}
extern "C" void darwin_audio_track_start(DarwinAudioTrack*) {}
extern "C" void darwin_audio_track_stop(DarwinAudioTrack*) {}
extern "C" void darwin_audio_track_pause(DarwinAudioTrack*) {}
extern "C" void darwin_audio_track_flush(DarwinAudioTrack*) {}
extern "C" void darwin_audio_track_set_volume(DarwinAudioTrack*, float, float) {}
extern "C" int32_t darwin_audio_track_buffer_capacity_frames(DarwinAudioTrack*) {
  return 960;
}
extern "C" uint32_t darwin_audio_track_position(DarwinAudioTrack*) { return 0; }
extern "C" size_t darwin_audio_track_write(DarwinAudioTrack*, const void*,
                                             size_t size, bool) {
  return size;
}

int main() {
  JNINativeInterface functions{};
  functions.FindClass = &FindClass;
  functions.RegisterNatives = &RegisterNatives;
  functions.DeleteLocalRef = &DeleteLocalRef;
  functions.GetObjectClass = &GetObjectClass;
  functions.GetFieldID = &GetFieldID;
  functions.GetLongField = &GetLongField;
  functions.SetLongField = &SetLongField;
  JNIEnv env{&functions};

  assert(darwin_art::media::RegisterAudioSystemNatives(&env));
  assert(darwin_art::media::RegisterAudioTrackNatives(&env));
  assert(darwin_art::media::RegisterAudioProductStrategyNatives(&env));
  assert(darwin_art::media::RegisterAudioPortEventHandlerNatives(&env));
  assert((registration_order == std::vector<std::string>{
      "android/media/AudioSystem", "android/media/AudioTrack",
      "android/media/audiopolicy/AudioProductStrategy",
      "android/media/AudioPortEventHandler"}));

  auto new_session = Native<jint (*)(JNIEnv*, jclass)>(
      "android/media/AudioSystem", "newAudioSessionId", "()I");
  assert(new_session(&env, nullptr) == 1);
  assert(new_session(&env, nullptr) == 2);
  auto min_buffer = Native<jint (*)(JNIEnv*, jclass, jint, jint, jint)>(
      "android/media/AudioTrack", "native_get_min_buff_size", "(III)I");
  assert(min_buffer(&env, nullptr, 0, 2, 2) == -2);
  assert(min_buffer(&env, nullptr, 48000, 2, 2) == 4096);

  auto setup = Native<jint (*)(JNIEnv*, jobject, jobject, jobject, jintArray,
                               jint, jint, jint, jint, jint, jintArray, jobject,
                               jlong, jboolean, jint, jobject, jstring)>(
      "android/media/AudioTrack", "native_setup",
      "(Ljava/lang/Object;Ljava/lang/Object;[IIIIII[ILandroid/os/Parcel;JZILjava/lang/Object;Ljava/lang/String;)I");
  auto release = Native<void (*)(JNIEnv*, jobject)>(
      "android/media/AudioTrack", "native_release", "()V");
  TrackObject object;
  assert(setup(&env, &object, nullptr, nullptr, nullptr, 3, 2, 2, 128, 0,
               nullptr, nullptr, 0, JNI_FALSE, 0, nullptr, nullptr) == 0);
  assert(create_calls == 1 && object.native != 0);
  release(&env, &object);
  assert(object.native == 0 && destroy_calls == 1);
  release(&env, &object);
  assert(destroy_calls == 1);

  methods.clear();
  fail_registration = true;
  assert(!darwin_art::media::RegisterAudioTrackNatives(&env));
  assert(!darwin_art::media::RegisterAudioSystemNatives(&env));
  std::puts("audio-jni: PASS registration order/session IDs/AudioTrack ownership");
}
