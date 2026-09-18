#pragma once

#include <cstdint>
#include <jni.h>

namespace darwin_art::input {

// Parcel owns Binder/layout conversion. The channel adapter owns the returned
// local references and guest descriptor, including provider failure paths.
struct InputChannelParcelData {
  bool initialized = false;
  jobject token = nullptr;
  jstring name = nullptr;
  int endpoint_fd = -1;
};

struct InputChannelParcelBridge {
  uint32_t version = 1;
  bool (*read)(JNIEnv*, jobject, InputChannelParcelData*) = nullptr;
  // All arguments are borrowed; the Parcel provider duplicates endpoint_fd.
  // False is failure, not successful writing of an uninitialized channel.
  bool (*write)(JNIEnv*, jobject, bool initialized, jobject token,
                const char* name, int endpoint_fd) = nullptr;
};

}  // namespace darwin_art::input
