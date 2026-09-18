#include "fixture_input_guest_io.h"

#include <limits>
#include <thread>

namespace darwin_art_graphics_fixture {

namespace {

constexpr jint kWouldBlockErrno = 11;  // Android EAGAIN/EWOULDBLOCK.
constexpr jint kInterruptedErrno = 4;  // Android EINTR.

void DeleteLocal(JNIEnv* env, jobject object) {
  if (env != nullptr && object != nullptr) env->DeleteLocalRef(object);
}

void ThrowInvalidArgument(JNIEnv* env, const char* message) {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass exception = env->FindClass("java/lang/IllegalArgumentException");
  if (exception != nullptr && !env->ExceptionCheck()) {
    env->ThrowNew(exception, message);
  }
  DeleteLocal(env, exception);
}

}  // namespace

FixtureInputGuestIo::FixtureInputGuestIo(JNIEnv* env,
                                         jobject borrowed_producer_fd)
    : env_(env), owner_thread_(std::this_thread::get_id()) {
  if (env_ == nullptr || borrowed_producer_fd == nullptr ||
      env_->ExceptionCheck())
    return;

  producer_fd_ = env_->NewLocalRef(borrowed_producer_fd);
  if (producer_fd_ == nullptr || env_->ExceptionCheck()) return;
  os_class_ = env_->FindClass("android/system/Os");
  if (os_class_ == nullptr || env_->ExceptionCheck()) return;
  errno_class_ = env_->FindClass("android/system/ErrnoException");
  if (errno_class_ == nullptr || env_->ExceptionCheck())
    return;
  write_ = env_->GetStaticMethodID(
      os_class_, "write",
      "(Ljava/io/FileDescriptor;[BII)I");
  if (write_ == nullptr || env_->ExceptionCheck()) return;
  read_ = env_->GetStaticMethodID(
      os_class_, "read",
      "(Ljava/io/FileDescriptor;[BII)I");
  if (read_ == nullptr || env_->ExceptionCheck()) return;
  errno_field_ = env_->GetFieldID(errno_class_, "errno", "I");
  if (write_ == nullptr || read_ == nullptr || errno_field_ == nullptr ||
      env_->ExceptionCheck())
    return;
  valid_ = true;
}

FixtureInputGuestIo::~FixtureInputGuestIo() {
  // The adapter never owns producer_fd_. Deleting these local class refs is
  // the only teardown needed and does not invoke Java code or close the FD.
  DeleteLocal(env_, os_class_);
  DeleteLocal(env_, errno_class_);
  DeleteLocal(env_, producer_fd_);
  os_class_ = nullptr;
  errno_class_ = nullptr;
  producer_fd_ = nullptr;
}

FixtureExchangeIo FixtureInputGuestIo::port() const {
  if (!valid_ || !OnOwnerThread()) return {};
  return {&FixtureInputGuestIo::WriteCallback,
          &FixtureInputGuestIo::ReadCallback, const_cast<FixtureInputGuestIo*>(this)};
}

FixtureIoResult FixtureInputGuestIo::WriteCallback(void* context,
                                                   const void* bytes,
                                                   size_t count) {
  auto* adapter = static_cast<FixtureInputGuestIo*>(context);
  return adapter == nullptr ? FixtureIoResult{FixtureIoStatus::kTerminal, 0}
                             : adapter->Write(bytes, count);
}

FixtureIoResult FixtureInputGuestIo::ReadCallback(void* context, void* bytes,
                                                  size_t count) {
  auto* adapter = static_cast<FixtureInputGuestIo*>(context);
  return adapter == nullptr ? FixtureIoResult{FixtureIoStatus::kTerminal, 0}
                             : adapter->Read(bytes, count);
}

bool FixtureInputGuestIo::OnOwnerThread() const {
  return owner_thread_ == std::this_thread::get_id();
}

FixtureIoResult FixtureInputGuestIo::ClassifyException() {
  if (env_ == nullptr || !env_->ExceptionCheck())
    return {FixtureIoStatus::kTerminal, 0};
  jthrowable original = env_->ExceptionOccurred();
  if (original == nullptr) {
    // A missing exception object is a JNI failure, not permission to treat the
    // operation as retryable. Leave the pending exception untouched.
    return {FixtureIoStatus::kTerminal, 0};
  }
  // JNI calls used to inspect the throwable cannot run while that throwable
  // is pending. Clear only for this bounded inspection, then restore the
  // original object for every non-retryable path.
  env_->ExceptionClear();
  bool retryable = false;
  if (errno_class_ != nullptr && errno_field_ != nullptr &&
      env_->IsInstanceOf(original, errno_class_) == JNI_TRUE &&
      !env_->ExceptionCheck()) {
    const jint error_number = env_->GetIntField(original, errno_field_);
    if (!env_->ExceptionCheck()) {
      retryable = error_number == kWouldBlockErrno ||
                  error_number == kInterruptedErrno;
    }
  }
  if (env_->ExceptionCheck()) {
    // Discard only a nested inspection failure; the operation's original
    // exception remains the one visible to the caller.
    jthrowable nested = env_->ExceptionOccurred();
    env_->ExceptionClear();
    DeleteLocal(env_, nested);
    env_->Throw(original);
    DeleteLocal(env_, original);
    return {FixtureIoStatus::kTerminal, 0};
  }
  if (retryable) {
    DeleteLocal(env_, original);
    return {FixtureIoStatus::kWouldBlock, 0};
  }
  // Non-retryable ErrnoException, like every other Java exception, is restored
  // for the caller. Do not clear or replace it.
  env_->Throw(original);
  DeleteLocal(env_, original);
  return {FixtureIoStatus::kTerminal, 0};
}

FixtureIoResult FixtureInputGuestIo::Write(const void* bytes, size_t count) {
  if (!valid_ || env_ == nullptr || !OnOwnerThread() || env_->ExceptionCheck())
    return {FixtureIoStatus::kTerminal, 0};
  if (count == 0) return {FixtureIoStatus::kProgress, 0};
  if (bytes == nullptr || count > static_cast<size_t>(std::numeric_limits<jint>::max())) {
    ThrowInvalidArgument(env_, "invalid fixture guest write buffer");
    return {FixtureIoStatus::kTerminal, 0};
  }
  jbyteArray array = env_->NewByteArray(static_cast<jsize>(count));
  if (array == nullptr || env_->ExceptionCheck()) {
    DeleteLocal(env_, array);
    return {FixtureIoStatus::kTerminal, 0};
  }
  env_->SetByteArrayRegion(array, 0, static_cast<jsize>(count),
                           static_cast<const jbyte*>(bytes));
  if (env_->ExceptionCheck()) {
    DeleteLocal(env_, array);
    return {FixtureIoStatus::kTerminal, 0};
  }
  const jint result = env_->CallStaticIntMethod(
      os_class_, write_, producer_fd_, array, 0, static_cast<jint>(count));
  DeleteLocal(env_, array);
  if (env_->ExceptionCheck()) return ClassifyException();
  if (result <= 0 || result > static_cast<jint>(count))
    return {FixtureIoStatus::kTerminal, 0};
  return {FixtureIoStatus::kProgress, static_cast<size_t>(result)};
}

FixtureIoResult FixtureInputGuestIo::Read(void* bytes, size_t count) {
  if (!valid_ || env_ == nullptr || !OnOwnerThread() || env_->ExceptionCheck())
    return {FixtureIoStatus::kTerminal, 0};
  if (count == 0) return {FixtureIoStatus::kProgress, 0};
  if (bytes == nullptr || count > static_cast<size_t>(std::numeric_limits<jint>::max())) {
    ThrowInvalidArgument(env_, "invalid fixture guest read buffer");
    return {FixtureIoStatus::kTerminal, 0};
  }
  jbyteArray array = env_->NewByteArray(static_cast<jsize>(count));
  if (array == nullptr || env_->ExceptionCheck()) {
    DeleteLocal(env_, array);
    return {FixtureIoStatus::kTerminal, 0};
  }
  const jint result = env_->CallStaticIntMethod(
      os_class_, read_, producer_fd_, array, 0, static_cast<jint>(count));
  if (env_->ExceptionCheck()) {
    DeleteLocal(env_, array);
    return ClassifyException();
  }
  if (result < 0 || result > static_cast<jint>(count)) {
    DeleteLocal(env_, array);
    return {FixtureIoStatus::kTerminal, 0};
  }
  if (result == 0) {
    DeleteLocal(env_, array);
    return {FixtureIoStatus::kTerminal, 0};  // EOF.
  }
  env_->GetByteArrayRegion(array, 0, result, static_cast<jbyte*>(bytes));
  DeleteLocal(env_, array);
  if (env_->ExceptionCheck()) return {FixtureIoStatus::kTerminal, 0};
  return {FixtureIoStatus::kProgress, static_cast<size_t>(result)};
}

}  // namespace darwin_art_graphics_fixture
