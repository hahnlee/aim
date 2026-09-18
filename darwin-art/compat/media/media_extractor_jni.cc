#include "media_extractor_jni.h"
#include "../darwin_media_extractor.h"
#include "../darwin_android_platform.h"
#include "../../_aosp/system/libziparchive/include/ziparchive/zip_archive.h"

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <iterator>
#include <limits>
#include <new>
#include <string>
#include <vector>

extern "C" intptr_t darwin_art_bionic_pread(int, void*, size_t, int64_t);

namespace darwin_art::media {
// MediaExtractor's Java class keeps its native handle in mNativeContext. The
// demux state and bounded WebM parser are shared with the NDK facade.

bool ExtractZipAsset(const std::string& source, std::vector<uint8_t>* bytes) {
  if (bytes == nullptr) return false;
  std::string apk = source;
  std::string entry;
  const size_t bang = source.find("!/");
  if (bang != std::string::npos) {
    apk = source.substr(0, bang);
    entry = source.substr(bang + 2);
    if (apk.rfind("jar:", 0) == 0) apk.erase(0, 4);
    if (apk.rfind("file://", 0) == 0) apk.erase(0, 7);
    else if (apk.rfind("file:", 0) == 0) apk.erase(0, 5);
  }
  if (entry.empty()) return false;
  ZipArchiveHandle archive = nullptr;
  if (OpenArchive(apk.c_str(), &archive) != 0) return false;
  ZipEntry64 zip_entry;
  const bool found = FindEntry(archive, entry, &zip_entry) == 0;
  if (found && zip_entry.uncompressed_length <= (64u * 1024u * 1024u)) {
    bytes->resize(static_cast<size_t>(zip_entry.uncompressed_length));
    if (ExtractToMemory(archive, &zip_entry, bytes->data(), bytes->size()) != 0)
      bytes->clear();
  }
  CloseArchive(archive);
  return !bytes->empty();
}

MediaExtractorState* GetExtractorState(JNIEnv* env, jobject self) {
  if (env == nullptr || self == nullptr) return nullptr;
  jclass type = env->GetObjectClass(self);
  jfieldID field = type == nullptr
                       ? nullptr
                       : env->GetFieldID(type, "mNativeContext", "J");
  jlong value = field == nullptr ? 0 : env->GetLongField(self, field);
  env->DeleteLocalRef(type);
  return reinterpret_cast<MediaExtractorState*>(static_cast<uintptr_t>(value));
}

void SetExtractorState(JNIEnv* env, jobject self, MediaExtractorState* state) {
  if (env == nullptr || self == nullptr) return;
  jclass type = env->GetObjectClass(self);
  jfieldID field = type == nullptr
                       ? nullptr
                       : env->GetFieldID(type, "mNativeContext", "J");
  if (field != nullptr) {
    env->SetLongField(self, field, reinterpret_cast<jlong>(state));
  }
  env->DeleteLocalRef(type);
}

jobject NewExtractorFormat(JNIEnv* env, const MediaExtractorState* state) {
  if (env == nullptr || state == nullptr) return nullptr;
  jclass map_class = env->FindClass("java/util/HashMap");
  if (map_class == nullptr) return nullptr;
  jmethodID constructor = env->GetMethodID(map_class, "<init>", "()V");
  jmethodID put = env->GetMethodID(
      map_class, "put", "(Ljava/lang/Object;Ljava/lang/Object;)Ljava/lang/Object;");
  jobject map = constructor == nullptr ? nullptr : env->NewObject(map_class, constructor);
  if (map == nullptr || put == nullptr) {
    env->DeleteLocalRef(map_class);
    return map;
  }
  jclass integer_class = env->FindClass("java/lang/Integer");
  jclass long_class = env->FindClass("java/lang/Long");
  jclass float_class = env->FindClass("java/lang/Float");
  jmethodID integer_value = integer_class == nullptr
                                ? nullptr
                                : env->GetMethodID(integer_class, "<init>", "(I)V");
  jmethodID long_value = long_class == nullptr
                             ? nullptr
                             : env->GetMethodID(long_class, "<init>", "(J)V");
  auto add_string = [&](const char* key, const char* value) {
    jstring k = env->NewStringUTF(key);
    jstring v = env->NewStringUTF(value);
    env->CallObjectMethod(map, put, k, v);
    env->DeleteLocalRef(k);
    env->DeleteLocalRef(v);
  };
  auto add_int = [&](const char* key, jint value) {
    jstring k = env->NewStringUTF(key);
    jobject v = integer_value == nullptr ? nullptr : env->NewObject(integer_class, integer_value, value);
    env->CallObjectMethod(map, put, k, v);
    env->DeleteLocalRef(k);
    env->DeleteLocalRef(v);
  };
  auto add_long = [&](const char* key, jlong value) {
    jstring k = env->NewStringUTF(key);
    jobject v = long_value == nullptr ? nullptr : env->NewObject(long_class, long_value, value);
    env->CallObjectMethod(map, put, k, v);
    env->DeleteLocalRef(k);
    env->DeleteLocalRef(v);
  };
  auto add_float = [&](const char* key, jfloat value) {
    jstring k = env->NewStringUTF(key);
    jmethodID ctor = float_class == nullptr ? nullptr : env->GetMethodID(float_class, "<init>", "(F)V");
    jobject v = ctor == nullptr ? nullptr : env->NewObject(float_class, ctor, value);
    env->CallObjectMethod(map, put, k, v);
    env->DeleteLocalRef(k);
    env->DeleteLocalRef(v);
  };
  add_string("mime", "video/x-vnd.on2.vp9");
  add_int("width", state->width);
  add_int("height", state->height);
  add_float("frame-rate", 30.0f);
  add_long("durationUs", state->duration_us);
  env->DeleteLocalRef(integer_class);
  env->DeleteLocalRef(long_class);
  env->DeleteLocalRef(float_class);
  env->DeleteLocalRef(map_class);
  return map;
}

void MediaExtractorNativeInit(JNIEnv*, jclass) {}
void MediaExtractorNativeSetup(JNIEnv* env, jobject self) {
  SetExtractorState(env, self, new (std::nothrow) MediaExtractorState());
}
void MediaExtractorNativeSetDataSource(JNIEnv* env, jobject self, jobject, jstring path,
                                       jobjectArray, jobjectArray) {
  auto* state = GetExtractorState(env, self);
  if (std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC") != nullptr) {
    std::cerr << "ART Android MediaExtractor: nativeSetDataSource state=" << state
              << " path_obj=" << path << "\n";
  }
  if (state == nullptr || path == nullptr) return;
  *state = MediaExtractorState{};
  const char* chars = env->GetStringUTFChars(path, nullptr);
  if (chars == nullptr) return;
  const std::string source(chars);
  std::vector<uint8_t> bytes;
  const bool parsed = ExtractZipAsset(chars, &bytes);
  env->ReleaseStringUTFChars(path, chars);
  if (parsed) ParseWebm(bytes, state);
  // Keep a normal filesystem path useful for non-APK callers.
  if (!state->has_source) {
    FILE* file = std::fopen(source.c_str(), "rb");
    if (file != nullptr) {
      std::fseek(file, 0, SEEK_END); const long length = std::ftell(file);
      std::fseek(file, 0, SEEK_SET);
      if (length > 0 && length <= 64 * 1024 * 1024) {
        bytes.resize(static_cast<size_t>(length));
        std::fread(bytes.data(), 1, bytes.size(), file);
        ParseWebm(bytes, state);
      }
      std::fclose(file);
    }
  }
  if (std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC") != nullptr) {
    std::cerr << "ART Android MediaExtractor: source=" << source
              << " parsed=" << parsed << " samples=" << state->samples.size()
              << " size=" << bytes.size() << " duration_us=" << state->duration_us
              << "\n";
  }
}
void MediaExtractorSetDataSourceFd(JNIEnv* env, jobject self, jobject descriptor,
                                   jlong offset, jlong length) {
  auto* state = GetExtractorState(env, self);
  if (std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC") != nullptr) {
    std::cerr << "ART Android MediaExtractor: setDataSourceFd state=" << state
              << " descriptor=" << descriptor << " offset=" << offset
              << " length=" << length << "\n";
  }
  if (state == nullptr || descriptor == nullptr) return;
  if (offset < 0 || length <= 0 ||
      static_cast<uint64_t>(length) > kMediaExtractorMaxBytes) {
    return;
  }
  jclass type = env->GetObjectClass(descriptor);
  jfieldID field = type == nullptr ? nullptr : env->GetFieldID(type, "descriptor", "I");
  if (field == nullptr && type != nullptr) {
    env->ExceptionClear();
    field = env->GetFieldID(type, "fd", "I");
  }
  const jint fd = field == nullptr ? -1 : env->GetIntField(descriptor, field);
  env->DeleteLocalRef(type);
  if (fd < 0) return;
  std::vector<uint8_t> bytes(static_cast<size_t>(length));
  size_t total = 0;
  while (total < bytes.size()) {
    if (offset > std::numeric_limits<jlong>::max() -
                    static_cast<jlong>(total)) {
      bytes.clear();
      break;
    }
    const intptr_t n = darwin_art_bionic_pread(
        fd, bytes.data() + total, bytes.size() - total,
        offset + static_cast<jlong>(total));
    if (n <= 0 || static_cast<uint64_t>(n) > bytes.size() - total) break;
    total += static_cast<size_t>(n);
  }
  bytes.resize(total);
  ParseWebm(bytes, state);
  if (std::getenv("DARWIN_ART_DEBUG_MEDIA_CODEC") != nullptr) {
    std::cerr << "ART Android MediaExtractor: fd=" << fd << " samples="
              << state->samples.size() << " bytes=" << bytes.size() << "\n";
  }
}
void MediaExtractorSetDataSourceMedia(JNIEnv* env, jobject self, jobject) {
  // Java MediaDataSource callbacks are not a filesystem source. Do not claim
  // a track unless the source was actually read and parsed.
  if (auto* state = GetExtractorState(env, self); state != nullptr)
    *state = MediaExtractorState{};
}
jobject MediaExtractorGetFileFormat(JNIEnv* env, jobject self) {
  return NewExtractorFormat(env, GetExtractorState(env, self));
}
jobject MediaExtractorGetTrackFormat(JNIEnv* env, jobject self, jint) {
  return NewExtractorFormat(env, GetExtractorState(env, self));
}
jint MediaExtractorGetTrackCount(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  return state != nullptr && state->has_source ? 1 : 0;
}
void MediaExtractorRelease(JNIEnv* env, jobject self) {
  delete GetExtractorState(env, self);
  SetExtractorState(env, self, nullptr);
}
void MediaExtractorFinalize(JNIEnv* env, jobject self) { MediaExtractorRelease(env, self); }
void MediaExtractorSelectTrack(JNIEnv*, jobject, jint) {}
void MediaExtractorUnselectTrack(JNIEnv*, jobject, jint) {}
jboolean MediaExtractorAdvance(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  if (state == nullptr || !state->has_source) return JNI_FALSE;
  if (state->sample_index + 1 >= state->samples.size()) {
    state->sample_index = state->samples.size();
    return JNI_FALSE;
  }
  ++state->sample_index;
  return JNI_TRUE;
}
jlong MediaExtractorGetSampleTime(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  return state != nullptr &&
                 state->sample_index < state->samples.size()
             ? state->samples[state->sample_index].pts_us : -1;
}
jlong MediaExtractorGetSampleSize(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  return state != nullptr &&
                 state->sample_index < state->samples.size()
             ? static_cast<jlong>(state->samples[state->sample_index].data.size()) : -1;
}
jint MediaExtractorGetSampleFlags(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  return state != nullptr &&
                 state->sample_index < state->samples.size()
             ? state->samples[state->sample_index].flags : 0;
}
jint MediaExtractorGetSampleTrackIndex(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  return state != nullptr && state->sample_index < state->samples.size() ? 0 : -1;
}
jint MediaExtractorReadSampleData(JNIEnv* env, jobject self, jobject buffer, jint offset) {
  auto* state = GetExtractorState(env, self);
  if (state == nullptr || buffer == nullptr ||
      state->sample_index >= state->samples.size() || offset < 0)
    return -1;
  void* destination = env->GetDirectBufferAddress(buffer);
  const jlong capacity = env->GetDirectBufferCapacity(buffer);
  const auto& sample = state->samples[state->sample_index].data;
  if (destination == nullptr ||
      sample.size() > static_cast<size_t>(std::numeric_limits<jint>::max()) ||
      static_cast<jlong>(offset) > capacity ||
      sample.size() > static_cast<size_t>(capacity - offset)) return -1;
  std::memcpy(static_cast<uint8_t*>(destination) + offset, sample.data(), sample.size());
  return static_cast<jint>(sample.size());
}
void MediaExtractorSeekTo(JNIEnv* env, jobject self, jlong time_us, jint mode) {
  auto* state = GetExtractorState(env, self);
  if (state == nullptr) return;
  size_t index = 0;
  while (index < state->samples.size() &&
         state->samples[index].pts_us < time_us) ++index;
  if (mode == 0 && index > 0) --index;
  state->sample_index = std::min(index, state->samples.size());
}
jlong MediaExtractorGetCachedDuration(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  return state == nullptr ? 0 : state->duration_us;
}
jboolean MediaExtractorHasCacheReachedEnd(JNIEnv* env, jobject self) {
  auto* state = GetExtractorState(env, self);
  return state == nullptr || state->sample_index + 1 >= state->samples.size();
}
jboolean MediaExtractorGetSampleCryptoInfo(JNIEnv*, jobject, jobject) { return JNI_FALSE; }
jobject MediaExtractorGetAudioPresentations(JNIEnv* env, jobject, jint) {
  jclass collections = env->FindClass("java/util/Collections");
  jmethodID empty = collections == nullptr ? nullptr : env->GetStaticMethodID(
      collections, "emptyList", "()Ljava/util/List;");
  jobject result = empty == nullptr ? nullptr : env->CallStaticObjectMethod(collections, empty);
  env->DeleteLocalRef(collections);
  return result;
}
jobject MediaExtractorGetMetrics(JNIEnv* env, jobject) {
  jclass type = env->FindClass("android/os/PersistableBundle");
  jmethodID ctor = type == nullptr ? nullptr : env->GetMethodID(type, "<init>", "()V");
  jobject result = ctor == nullptr ? nullptr : env->NewObject(type, ctor);
  env->DeleteLocalRef(type);
  return result;
}
void MediaExtractorSetLogSessionId(JNIEnv*, jobject, jstring) {}
void MediaExtractorSetMediaCas(JNIEnv*, jobject, jobject) {}
bool RegisterMediaExtractorNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("getFileFormatNative"),
       const_cast<char*>("()Ljava/util/Map;"),
       reinterpret_cast<void*>(&MediaExtractorGetFileFormat)},
      {const_cast<char*>("getTrackFormatNative"),
       const_cast<char*>("(I)Ljava/util/Map;"),
       reinterpret_cast<void*>(&MediaExtractorGetTrackFormat)},
      {const_cast<char*>("nativeSetDataSource"),
       const_cast<char*>(
           "(Landroid/os/IBinder;Ljava/lang/String;[Ljava/lang/String;"
           "[Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&MediaExtractorNativeSetDataSource)},
      {const_cast<char*>("nativeSetMediaCas"),
       const_cast<char*>("(Landroid/os/IHwBinder;)V"),
       reinterpret_cast<void*>(&MediaExtractorSetMediaCas)},
      {const_cast<char*>("native_finalize"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&MediaExtractorFinalize)},
      {const_cast<char*>("native_getAudioPresentations"),
       const_cast<char*>("(I)Ljava/util/List;"),
       reinterpret_cast<void*>(&MediaExtractorGetAudioPresentations)},
      {const_cast<char*>("native_getMetrics"),
       const_cast<char*>("()Landroid/os/PersistableBundle;"),
       reinterpret_cast<void*>(&MediaExtractorGetMetrics)},
      {const_cast<char*>("native_init"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&MediaExtractorNativeInit)},
      {const_cast<char*>("native_setLogSessionId"),
       const_cast<char*>("(Ljava/lang/String;)V"),
       reinterpret_cast<void*>(&MediaExtractorSetLogSessionId)},
      {const_cast<char*>("native_setup"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&MediaExtractorNativeSetup)},
      {const_cast<char*>("advance"), const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&MediaExtractorAdvance)},
      {const_cast<char*>("getCachedDuration"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&MediaExtractorGetCachedDuration)},
      {const_cast<char*>("getSampleCryptoInfo"),
       const_cast<char*>("(Landroid/media/MediaCodec$CryptoInfo;)Z"),
       reinterpret_cast<void*>(&MediaExtractorGetSampleCryptoInfo)},
      {const_cast<char*>("getSampleFlags"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&MediaExtractorGetSampleFlags)},
      {const_cast<char*>("getSampleSize"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&MediaExtractorGetSampleSize)},
      {const_cast<char*>("getSampleTime"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&MediaExtractorGetSampleTime)},
      {const_cast<char*>("getSampleTrackIndex"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&MediaExtractorGetSampleTrackIndex)},
      {const_cast<char*>("setDataSource"),
       const_cast<char*>("(Ljava/io/FileDescriptor;JJ)V"),
       reinterpret_cast<void*>(&MediaExtractorSetDataSourceFd)},
      {const_cast<char*>("setDataSource"),
       const_cast<char*>("(Landroid/media/MediaDataSource;)V"),
       reinterpret_cast<void*>(&MediaExtractorSetDataSourceMedia)},
      {const_cast<char*>("getTrackCount"), const_cast<char*>("()I"),
       reinterpret_cast<void*>(&MediaExtractorGetTrackCount)},
      {const_cast<char*>("hasCacheReachedEndOfStream"),
       const_cast<char*>("()Z"),
       reinterpret_cast<void*>(&MediaExtractorHasCacheReachedEnd)},
      {const_cast<char*>("readSampleData"),
       const_cast<char*>("(Ljava/nio/ByteBuffer;I)I"),
       reinterpret_cast<void*>(&MediaExtractorReadSampleData)},
      {const_cast<char*>("release"), const_cast<char*>("()V"),
       reinterpret_cast<void*>(&MediaExtractorRelease)},
      {const_cast<char*>("seekTo"), const_cast<char*>("(JI)V"),
       reinterpret_cast<void*>(&MediaExtractorSeekTo)},
      {const_cast<char*>("selectTrack"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&MediaExtractorSelectTrack)},
      {const_cast<char*>("unselectTrack"), const_cast<char*>("(I)V"),
       reinterpret_cast<void*>(&MediaExtractorUnselectTrack)},
  };
  if (env == nullptr) return false;
  jclass clazz = env->FindClass("android/media/MediaExtractor");
  if (clazz == nullptr) return false;
  const bool registered =
      env->RegisterNatives(clazz, methods, static_cast<jint>(std::size(methods))) ==
      JNI_OK;
  env->DeleteLocalRef(clazz);
  return registered;
}

}  // namespace darwin_art::media
