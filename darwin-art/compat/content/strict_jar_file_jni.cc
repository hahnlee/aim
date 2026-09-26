#include "strict_jar_file_jni.h"

#include "../../_aosp/system/libziparchive/include/ziparchive/zip_archive.h"

#include <fcntl.h>
#include <unistd.h>

#include <cstdint>
#include <iterator>
#include <string>

extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int fd, int* host_fd);

// Port of AOSP android_util_jar_StrictJarFile.cpp. The one difference is the
// descriptor: Java holds an Android descriptor of this process, so the
// archive reads a host duplicate that it owns, where AOSP reads the Java
// descriptor without owning it.
namespace darwin_art::content {
namespace {

jclass g_zip_entry_class;

void ThrowIoException(JNIEnv* env, const char* message) {
  jclass exception = env->FindClass("java/io/IOException");
  if (exception != nullptr) env->ThrowNew(exception, message);
}

class Utf {
 public:
  Utf(JNIEnv* env, jstring value)
      : env_(env), value_(value),
        chars_(value == nullptr ? nullptr : env->GetStringUTFChars(value, nullptr)) {
    if (value == nullptr) {
      jclass npe = env->FindClass("java/lang/NullPointerException");
      if (npe != nullptr) env->ThrowNew(npe, nullptr);
    }
  }
  ~Utf() {
    if (chars_ != nullptr) env_->ReleaseStringUTFChars(value_, chars_);
  }
  const char* c_str() const { return chars_; }

 private:
  JNIEnv* env_;
  jstring value_;
  const char* chars_;
};
// ZipEntry.<init>(String,String,JJJII[BJ)
jmethodID g_zip_entry_ctor;

void ThrowIoException(JNIEnv* env, int32_t error) {
  ThrowIoException(env, ErrorCodeString(error));
}

jobject NewZipEntry(JNIEnv* env, const ZipEntry& entry, jstring name) {
  return env->NewObject(g_zip_entry_class, g_zip_entry_ctor, name,
                        nullptr,  // comment
                        static_cast<jlong>(entry.crc32),
                        static_cast<jlong>(entry.compressed_length),
                        static_cast<jlong>(entry.uncompressed_length),
                        static_cast<jint>(entry.method),
                        static_cast<jint>(0),  // time
                        nullptr,               // extra
                        static_cast<jlong>(entry.offset));
}

jlong OpenJarFile(JNIEnv* env, jclass, jstring name, jint fd) {
  // The name is used for logging only.
  Utf name_chars(env, name);
  if (name_chars.c_str() == nullptr) return -1;
  int host_fd = -1;
  if (darwin_art_bionic_fs_dup_host_fd_core(fd, &host_fd) != 1) {
    ThrowIoException(env, "Invalid file descriptor");
    return -1;
  }
  fcntl(host_fd, F_SETFD, FD_CLOEXEC);
  ZipArchiveHandle handle;
  const int32_t error =
      OpenArchiveFd(host_fd, name_chars.c_str(), &handle, /*assume_ownership=*/true);
  if (error != 0) {
    CloseArchive(handle);
    ThrowIoException(env, error);
    return -1;
  }
  return reinterpret_cast<jlong>(handle);
}

class IterationHandle {
 public:
  void** CookieAddress() { return &cookie_; }
  ~IterationHandle() { EndIteration(cookie_); }

 private:
  void* cookie_ = nullptr;
};

jlong StartJarIteration(JNIEnv* env, jclass, jlong native_handle, jstring prefix) {
  Utf prefix_chars(env, prefix);
  if (prefix_chars.c_str() == nullptr) return -1;
  auto* handle = new IterationHandle();
  const int32_t error =
      StartIteration(reinterpret_cast<ZipArchiveHandle>(native_handle),
                     handle->CookieAddress(), prefix_chars.c_str(), "");
  if (error != 0) {
    delete handle;
    ThrowIoException(env, error);
    return -1;
  }
  return reinterpret_cast<jlong>(handle);
}

jobject NextJarEntry(JNIEnv* env, jclass, jlong iteration_handle) {
  ZipEntry data;
  std::string name;
  auto* handle = reinterpret_cast<IterationHandle*>(iteration_handle);
  if (Next(*handle->CookieAddress(), &data, &name) != 0) {
    delete handle;
    return nullptr;
  }
  jstring name_string = env->NewStringUTF(name.c_str());
  if (name_string == nullptr) return nullptr;
  jobject entry = NewZipEntry(env, data, name_string);
  env->DeleteLocalRef(name_string);
  return entry;
}

jobject FindJarEntry(JNIEnv* env, jclass, jlong native_handle, jstring name) {
  Utf name_chars(env, name);
  if (name_chars.c_str() == nullptr) return nullptr;
  ZipEntry data;
  if (FindEntry(reinterpret_cast<ZipArchiveHandle>(native_handle), name_chars.c_str(),
                &data) != 0) {
    return nullptr;
  }
  return NewZipEntry(env, data, name);
}

void CloseJarFile(JNIEnv*, jclass, jlong native_handle) {
  CloseArchive(reinterpret_cast<ZipArchiveHandle>(native_handle));
}

}  // namespace

bool RegisterStrictJarFileNatives(JNIEnv* env) {
  jclass zip_entry = env->FindClass("java/util/zip/ZipEntry");
  if (zip_entry == nullptr) return false;
  g_zip_entry_class = static_cast<jclass>(env->NewGlobalRef(zip_entry));
  env->DeleteLocalRef(zip_entry);
  g_zip_entry_ctor = env->GetMethodID(g_zip_entry_class, "<init>",
                                      "(Ljava/lang/String;Ljava/lang/String;JJJII[BJ)V");
  if (g_zip_entry_ctor == nullptr) return false;
  const JNINativeMethod methods[] = {
      {"nativeOpenJarFile", "(Ljava/lang/String;I)J",
       reinterpret_cast<void*>(&OpenJarFile)},
      {"nativeStartIteration", "(JLjava/lang/String;)J",
       reinterpret_cast<void*>(&StartJarIteration)},
      {"nativeNextEntry", "(J)Ljava/util/zip/ZipEntry;",
       reinterpret_cast<void*>(&NextJarEntry)},
      {"nativeFindEntry", "(JLjava/lang/String;)Ljava/util/zip/ZipEntry;",
       reinterpret_cast<void*>(&FindJarEntry)},
      {"nativeClose", "(J)V", reinterpret_cast<void*>(&CloseJarFile)},
  };
  jclass strict_jar_file = env->FindClass("android/util/jar/StrictJarFile");
  if (strict_jar_file == nullptr) return false;
  const bool registered =
      env->RegisterNatives(strict_jar_file, methods,
                           static_cast<jint>(std::size(methods))) == JNI_OK;
  env->DeleteLocalRef(strict_jar_file);
  return registered;
}

}  // namespace darwin_art::content
