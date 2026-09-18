#pragma once

#include "input_channel_parcel.h"
#include <utility>

extern "C" int darwin_art_bionic_socket_broker_close(int);

namespace darwin_art::input {
// JNI resources are scoped to a native call; endpoint/policy owners never
// retain these borrowed strings or Parcel local references.
class InputChannelUtfChars final {
 public:
  InputChannelUtfChars(JNIEnv* env, jstring name) : env_(env), name_(name),
      value_(name == nullptr ? nullptr : env->GetStringUTFChars(name, nullptr)) {}
  ~InputChannelUtfChars() {
    if (value_ != nullptr) env_->ReleaseStringUTFChars(name_, value_);
  }
  InputChannelUtfChars(const InputChannelUtfChars&) = delete;
  InputChannelUtfChars& operator=(const InputChannelUtfChars&) = delete;
  const char* Get() const { return value_; }
 private:
  JNIEnv* env_;
  jstring name_;
  const char* value_;
};
class InputChannelParcelResources final {
 public:
  InputChannelParcelResources(JNIEnv* env, InputChannelParcelData& data)
      : env_(env), data_(data) {}
  ~InputChannelParcelResources() {
    if (data_.token != nullptr) env_->DeleteLocalRef(std::exchange(data_.token, nullptr));
    if (data_.name != nullptr) env_->DeleteLocalRef(std::exchange(data_.name, nullptr));
    if (data_.endpoint_fd >= 0)
      (void)darwin_art_bionic_socket_broker_close(std::exchange(data_.endpoint_fd, -1));
  }
  InputChannelParcelResources(const InputChannelParcelResources&) = delete;
  InputChannelParcelResources& operator=(const InputChannelParcelResources&) = delete;
 private:
  JNIEnv* env_;
  InputChannelParcelData& data_;
};
inline void ThrowInputChannelOutOfMemory(JNIEnv* env) {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/OutOfMemoryError");
  if (type != nullptr) {
    (void)env->ThrowNew(type, "Unable to allocate native InputChannel resources");
    env->DeleteLocalRef(type);
  }
}
}  // namespace darwin_art::input
