#pragma once

#include <jni.h>

#include <thread>

#include "fixture_input_exchange.h"

namespace darwin_art_graphics_fixture {

// TEST ONLY: a scoped bridge from the fixture's borrowed producer
// FileDescriptor to the guest android.system.Os byte-stream API. The object
// and the FixtureExchangeIo returned by port() must stay on the constructing
// ART/Looper thread and within the native invocation that supplied env.
// Nothing here owns or closes the descriptor. The borrowed handle is pinned by
// one scoped local reference so a reentrant JNI call cannot invalidate it, and
// no JNIEnv is retained by fixture state after this adapter is destroyed.
class FixtureInputGuestIo final {
 public:
  FixtureInputGuestIo(JNIEnv* env, jobject borrowed_producer_fd);
  ~FixtureInputGuestIo();

  FixtureInputGuestIo(const FixtureInputGuestIo&) = delete;
  FixtureInputGuestIo& operator=(const FixtureInputGuestIo&) = delete;
  FixtureInputGuestIo(FixtureInputGuestIo&&) = delete;
  FixtureInputGuestIo& operator=(FixtureInputGuestIo&&) = delete;

  bool IsValid() const { return valid_; }
  FixtureExchangeIo port() const;

 private:
  static FixtureIoResult WriteCallback(void* context, const void* bytes,
                                       size_t count);
  static FixtureIoResult ReadCallback(void* context, void* bytes, size_t count);

  FixtureIoResult Write(const void* bytes, size_t count);
  FixtureIoResult Read(void* bytes, size_t count);
  FixtureIoResult ClassifyException();
  bool OnOwnerThread() const;

  JNIEnv* env_ = nullptr;
  jobject producer_fd_ = nullptr;  // borrowed; never closed here
  jclass os_class_ = nullptr;      // scoped local reference
  jclass errno_class_ = nullptr;   // scoped local reference
  jmethodID write_ = nullptr;
  jmethodID read_ = nullptr;
  jfieldID errno_field_ = nullptr;
  std::thread::id owner_thread_;
  bool valid_ = false;
};

}  // namespace darwin_art_graphics_fixture
