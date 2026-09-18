#pragma once

#include <jni.h>

#include <thread>

namespace darwin_art_graphics_fixture {

// TEST ONLY: owns the producer side of a genuinely imported InputChannel and
// the receiver-specific Java objects used by the graphics fixture.  The
// channel is intentionally not installed in any mutable ViewRoot receiver
// field; WindowInputEventReceiver registers itself against the InputChannel
// supplied to its constructor.
//
// All methods must be called from the ART/Looper owner thread.  This object
// does not retain a JNIEnv (and therefore cannot be disposed from a destructor
// on an arbitrary thread).
class FixtureInputChannel final {
 public:
  FixtureInputChannel() = default;
  ~FixtureInputChannel() = default;

  FixtureInputChannel(const FixtureInputChannel&) = delete;
  FixtureInputChannel& operator=(const FixtureInputChannel&) = delete;
  FixtureInputChannel(FixtureInputChannel&&) = delete;
  FixtureInputChannel& operator=(FixtureInputChannel&&) = delete;

  // Creates a fresh AF_UNIX stream pair, serializes fd_b using the framework's
  // InputChannel parcel contract, and constructs a real
  // ViewRootImpl.WindowInputEventReceiver on owner_looper.  view_root is only
  // the constructor's outer instance; this function never writes a ViewRoot
  // receiver field.
  bool Initialize(JNIEnv* env, jobject view_root, jobject owner_looper,
                  const char* channel_name);

  // Disposes the Java receiver/channel and closes both original socketpair
  // descriptors exactly once.  A pending JNI exception is saved and restored
  // after cleanup; cleanup failures are retained only when no exception was
  // already pending.  An instance is one-shot, including after a failed
  // initialization, so closed-resource state can never be reused for a new
  // descriptor pair.
  void Dispose(JNIEnv* env);

  bool IsInitialized() const { return receiver_ != nullptr; }
  jobject producer_fd() const { return producer_fd_; }
  jobject receiver_fd() const { return receiver_fd_; }
  jobject receiver() const { return receiver_; }
  jobject channel() const { return channel_; }
  jobject view_root() const { return view_root_; }
  jobject token() const { return token_; }

 private:
  jobject producer_fd_ = nullptr;  // socketpair fd_a, retained for transport
  jobject receiver_fd_ = nullptr;  // original fd_b passed through the Parcel
  jobject receiver_ = nullptr;
  jobject channel_ = nullptr;
  jobject view_root_ = nullptr;
  jobject token_ = nullptr;
  std::thread::id owner_thread_;
  bool initialized_once_ = false;
  bool producer_fd_closed_ = false;
  bool receiver_fd_closed_ = false;
  bool receiver_disposed_ = false;
  bool channel_disposed_ = false;
};

}  // namespace darwin_art_graphics_fixture
