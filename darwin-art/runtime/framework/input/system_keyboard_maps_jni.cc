#include "system_keyboard_maps_jni.h"

#include <input/KeyCharacterMap.h>
#include <nativehelper/ScopedUtfChars.h>

#include <memory>
#include <new>
#include <string>

#include "../../../compat/filesystem/guest_config.h"

namespace android {
// Original framework JNI factory: it owns map lifetime and the Java native
// pointer. Do not manufacture a second native map object or Parcel format.
jobject android_view_KeyCharacterMap_create(
    JNIEnv*, int32_t device_id, std::unique_ptr<KeyCharacterMap> map);
}

namespace darwin_art::input {
namespace {
void FailLoad(JNIEnv* env, const std::string& message) {
  if (env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/IllegalStateException");
  if (type != nullptr) {
    env->ThrowNew(type, message.c_str());
    env->DeleteLocalRef(type);
  }
}

jobject LoadMap(JNIEnv* env, jint device_id, jstring path_object) {
  if (path_object == nullptr) {
    FailLoad(env, "System keyboard map path is null");
    return nullptr;
  }
  ScopedUtfChars path_chars(env, path_object);
  if (path_chars.c_str() == nullptr) return nullptr;
  const std::string path(path_chars.c_str());
  std::string contents;
  // Use the guest filesystem's exact open-file description. Original host
  // Tokenizer::open would incorrectly interpret /system as a macOS pathname.
  if (!darwin_art::filesystem::ReadGuestConfig(path, &contents)) {
    FailLoad(env, "Cannot read system keyboard map: " + path);
    return nullptr;
  }
  if (contents.find('\0') != std::string::npos) {
    FailLoad(env, "Embedded NUL in system keyboard map: " + path);
    return nullptr;
  }
  auto parsed = android::KeyCharacterMap::loadContents(
      path, contents.c_str(), android::KeyCharacterMap::Format::BASE);
  if (!parsed.ok()) {
    FailLoad(env, "Invalid system keyboard map " + path + ": " +
                      parsed.error().message());
    return nullptr;
  }
  return android::android_view_KeyCharacterMap_create(
      env, device_id, std::make_unique<android::KeyCharacterMap>(*parsed.value()));
}

jobject Load(JNIEnv* env, jclass, jint device_id, jstring path_object) {
  if (env->ExceptionCheck()) return nullptr;
  try {
    return LoadMap(env, device_id, path_object);
  } catch (const std::bad_alloc&) {
    if (!env->ExceptionCheck()) {
      jclass type = env->FindClass("java/lang/OutOfMemoryError");
      if (type != nullptr) {
        env->ThrowNew(type, "Cannot allocate system keyboard map");
        env->DeleteLocalRef(type);
      }
    }
    return nullptr;
  }
}
}  // namespace

bool RegisterSystemKeyboardMapsNatives(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  jclass owner = env->FindClass("dev/darwinart/runtime/input/SystemKeyboardMaps");
  if (owner == nullptr) return false;
  const JNINativeMethod method{
      const_cast<char*>("nativeLoad"),
      const_cast<char*>("(ILjava/lang/String;)Landroid/view/KeyCharacterMap;"),
      reinterpret_cast<void*>(&Load)};
  const bool registered = env->RegisterNatives(owner, &method, 1) == JNI_OK;
  env->DeleteLocalRef(owner);
  return registered;
}
}  // namespace darwin_art::input
