#include "installd_jni.h"

#include <cstdint>
#include <cstdlib>

extern "C" int darwin_art_runtime_installd_create_app_data(
    const char* socket, const char* package, uint32_t user, uint32_t flags,
    uint64_t* ce_inode, uint64_t* de_inode, uint32_t* errno_out);
extern "C" int darwin_art_runtime_installd_app_data(
    const char* socket, const char* package, uint32_t user, uint32_t flags,
    int32_t clear, uint32_t* errno_out);
extern "C" int darwin_art_runtime_installd_rm_package_dir(
    const char* socket, const char* code_path, uint32_t* errno_out);

namespace darwin_art::framework::pm {
namespace {
constexpr jint kInvalid = -1;

const char* Socket() {
  const char* socket = std::getenv("DARWIN_ART_PROFILE_SOCKET");
  return socket != nullptr && *socket != '\0' ? socket : nullptr;
}

// Holds a Java string's UTF-8 for the duration of one native call.
class Utf {
 public:
  Utf(JNIEnv* env, jstring value)
      : env_(env), value_(value),
        text_(value == nullptr ? nullptr : env->GetStringUTFChars(value, nullptr)) {}
  ~Utf() { if (text_ != nullptr) env_->ReleaseStringUTFChars(value_, text_); }
  const char* get() const { return text_; }
 private:
  JNIEnv* env_;
  jstring value_;
  const char* text_;
};

void StoreErrno(JNIEnv* env, jintArray errno_out, uint32_t value) {
  const jint code = static_cast<jint>(value);
  env->SetIntArrayRegion(errno_out, 0, 1, &code);
}

jint CreateAppData(JNIEnv* env, jclass, jstring package, jint user, jint flags,
                   jlongArray inodes, jintArray errno_out) {
  Utf name(env, package);
  if (Socket() == nullptr || name.get() == nullptr || user < 0 || inodes == nullptr ||
      env->GetArrayLength(inodes) != 2 || errno_out == nullptr ||
      env->GetArrayLength(errno_out) != 1) {
    return kInvalid;
  }
  uint64_t ce = 0, de = 0;
  uint32_t code = 0;
  const int status = darwin_art_runtime_installd_create_app_data(
      Socket(), name.get(), static_cast<uint32_t>(user), static_cast<uint32_t>(flags),
      &ce, &de, &code);
  const jlong values[2] = {static_cast<jlong>(ce), static_cast<jlong>(de)};
  env->SetLongArrayRegion(inodes, 0, 2, values);
  StoreErrno(env, errno_out, code);
  return status;
}

jint AppData(JNIEnv* env, jclass, jstring package, jint user, jint flags, jboolean clear,
             jintArray errno_out) {
  Utf name(env, package);
  if (Socket() == nullptr || name.get() == nullptr || user < 0 || errno_out == nullptr ||
      env->GetArrayLength(errno_out) != 1) {
    return kInvalid;
  }
  uint32_t code = 0;
  const int status = darwin_art_runtime_installd_app_data(
      Socket(), name.get(), static_cast<uint32_t>(user), static_cast<uint32_t>(flags),
      clear == JNI_TRUE ? 1 : 0, &code);
  StoreErrno(env, errno_out, code);
  return status;
}

jint RmPackageDir(JNIEnv* env, jclass, jstring code_path, jintArray errno_out) {
  Utf path(env, code_path);
  if (Socket() == nullptr || path.get() == nullptr || errno_out == nullptr ||
      env->GetArrayLength(errno_out) != 1) {
    return kInvalid;
  }
  uint32_t code = 0;
  const int status = darwin_art_runtime_installd_rm_package_dir(Socket(), path.get(), &code);
  StoreErrno(env, errno_out, code);
  return status;
}
}  // namespace

bool RegisterDarwinInstalld(JNIEnv* env, jclass installd) {
  if (env == nullptr || installd == nullptr || env->ExceptionCheck()) return false;
  const JNINativeMethod methods[] = {
      {const_cast<char*>("nativeCreateAppData"),
       const_cast<char*>("(Ljava/lang/String;II[J[I)I"),
       reinterpret_cast<void*>(CreateAppData)},
      {const_cast<char*>("nativeAppData"), const_cast<char*>("(Ljava/lang/String;IIZ[I)I"),
       reinterpret_cast<void*>(AppData)},
      {const_cast<char*>("nativeRmPackageDir"), const_cast<char*>("(Ljava/lang/String;[I)I"),
       reinterpret_cast<void*>(RmPackageDir)},
  };
  return env->RegisterNatives(installd, methods, 3) == JNI_OK;
}
}  // namespace darwin_art::framework::pm
