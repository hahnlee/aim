#include "fixture_input_channel.h"

#include <thread>

namespace darwin_art_graphics_fixture {

namespace {

constexpr jint kAfUnix = 1;
constexpr jint kSockStream = 1;
constexpr jint kSockNonblock = 0x800;

class LocalRef final {
 public:
  LocalRef(JNIEnv* env, jobject value) : env_(env), value_(value) {}
  ~LocalRef() {
    if (env_ != nullptr && value_ != nullptr) env_->DeleteLocalRef(value_);
  }
  LocalRef(const LocalRef&) = delete;
  LocalRef& operator=(const LocalRef&) = delete;
  jobject get() const { return value_; }

 private:
  JNIEnv* env_;
  jobject value_;
};

class LocalClass final {
 public:
  LocalClass(JNIEnv* env, jclass value) : env_(env), value_(value) {}
  ~LocalClass() {
    if (env_ != nullptr && value_ != nullptr) env_->DeleteLocalRef(value_);
  }
  LocalClass(const LocalClass&) = delete;
  LocalClass& operator=(const LocalClass&) = delete;
  jclass get() const { return value_; }

 private:
  JNIEnv* env_;
  jclass value_;
};

void ThrowNew(JNIEnv* env, const char* class_name, const char* message) {
  if (env == nullptr || env->ExceptionCheck()) return;
  LocalClass exception(env, env->FindClass(class_name));
  if (exception.get() != nullptr && !env->ExceptionCheck())
    env->ThrowNew(exception.get(), message);
}

// Cleanup can itself call Java and throw. Keep the first exception (the
// caller's pending exception wins) while allowing all independent resources
// to be attempted.
class ExceptionPreserver final {
 public:
  explicit ExceptionPreserver(JNIEnv* env) : env_(env) {
    if (env_ != nullptr && env_->ExceptionCheck()) {
      original_ = env_->ExceptionOccurred();
      env_->ExceptionClear();
    }
  }
  ~ExceptionPreserver() {
    if (env_ == nullptr) return;
    jthrowable restore = original_ != nullptr ? original_ : cleanup_;
    if (restore != nullptr) env_->Throw(restore);
    if (original_ != nullptr) env_->DeleteLocalRef(original_);
    if (cleanup_ != nullptr) env_->DeleteLocalRef(cleanup_);
  }
  ExceptionPreserver(const ExceptionPreserver&) = delete;
  ExceptionPreserver& operator=(const ExceptionPreserver&) = delete;

  void Capture() {
    if (env_ == nullptr || !env_->ExceptionCheck()) return;
    jthrowable thrown = env_->ExceptionOccurred();
    env_->ExceptionClear();
    if (original_ != nullptr || cleanup_ != nullptr) {
      if (thrown != nullptr) env_->DeleteLocalRef(thrown);
      return;
    }
    cleanup_ = thrown;
  }

 private:
  JNIEnv* env_;
  jthrowable original_ = nullptr;
  jthrowable cleanup_ = nullptr;
};

bool MakeGlobal(JNIEnv* env, jobject local, jobject* destination) {
  if (env == nullptr || local == nullptr || destination == nullptr) return false;
  jobject global = env->NewGlobalRef(local);
  if (global == nullptr) {
    if (!env->ExceptionCheck()) {
      ThrowNew(env, "java/lang/OutOfMemoryError",
               "fixture InputChannel could not retain a Java reference");
    }
    return false;
  }
  if (env->ExceptionCheck()) {
    // A provider must not leave a newly-created global reference orphaned if
    // it reports an exception alongside the result.
    env->DeleteGlobalRef(global);
    return false;
  }
  *destination = global;
  return true;
}

void CloseLocalDescriptors(JNIEnv* env, jclass os_class, jmethodID close,
                           jobject fd_a, jobject fd_b) {
  if (env == nullptr || os_class == nullptr || close == nullptr) return;
  ExceptionPreserver exceptions(env);
  if (fd_a != nullptr) {
    env->CallStaticVoidMethod(os_class, close, fd_a);
    exceptions.Capture();
  }
  if (fd_b != nullptr) {
    env->CallStaticVoidMethod(os_class, close, fd_b);
    exceptions.Capture();
  }
}

void DisposeLocalObject(JNIEnv* env, jobject object, const char* method_name) {
  if (env == nullptr || object == nullptr) return;
  ExceptionPreserver exceptions(env);
  LocalClass object_class(env, env->GetObjectClass(object));
  jmethodID dispose = object_class.get() == nullptr || env->ExceptionCheck()
                          ? nullptr
                          : env->GetMethodID(object_class.get(), method_name,
                                             "()V");
  if (dispose == nullptr || env->ExceptionCheck()) {
    exceptions.Capture();
    return;
  }
  env->CallVoidMethod(object, dispose);
  exceptions.Capture();
}

}  // namespace

bool FixtureInputChannel::Initialize(JNIEnv* env, jobject view_root,
                                     jobject owner_looper,
                                     const char* channel_name) {
  if (env == nullptr) return false;
  if (view_root == nullptr || owner_looper == nullptr || channel_name == nullptr) {
    ThrowNew(env, "java/lang/IllegalArgumentException",
             "fixture InputChannel requires root, looper, and name");
    return false;
  }
  if (env->ExceptionCheck() || initialized_once_) return false;
  initialized_once_ = true;
  owner_thread_ = std::this_thread::get_id();

  LocalClass fd_class(env, env->FindClass("java/io/FileDescriptor"));
  jmethodID fd_ctor = fd_class.get() == nullptr || env->ExceptionCheck()
                          ? nullptr
                          : env->GetMethodID(fd_class.get(), "<init>", "()V");
  LocalRef fd_a(env, fd_ctor == nullptr || env->ExceptionCheck()
                         ? nullptr
                         : env->NewObject(fd_class.get(), fd_ctor));
  LocalRef fd_b(env, fd_ctor == nullptr || env->ExceptionCheck()
                         ? nullptr
                         : env->NewObject(fd_class.get(), fd_ctor));
  if (fd_a.get() == nullptr || fd_b.get() == nullptr || env->ExceptionCheck())
    return false;

  LocalClass os_class(env, env->FindClass("android/system/Os"));
  jmethodID socketpair =
      os_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetStaticMethodID(
                os_class.get(), "socketpair",
                "(IIILjava/io/FileDescriptor;Ljava/io/FileDescriptor;)V");
  jmethodID close = os_class.get() == nullptr || env->ExceptionCheck()
                        ? nullptr
                        : env->GetStaticMethodID(
                              os_class.get(), "close",
                              "(Ljava/io/FileDescriptor;)V");
  if (socketpair == nullptr || close == nullptr || env->ExceptionCheck())
    return false;
  env->CallStaticVoidMethod(os_class.get(), socketpair, kAfUnix,
                            kSockStream | kSockNonblock, 0, fd_a.get(),
                            fd_b.get());
  if (env->ExceptionCheck()) {
    // Os.socketpair is expected to be all-or-nothing, but close both Java
    // descriptors on the failure path as well in case a provider assigned one
    // endpoint before reporting its error.
    CloseLocalDescriptors(env, os_class.get(), close, fd_a.get(), fd_b.get());
    return false;
  }

  // Promote each descriptor independently. If the second promotion fails,
  // fd_b is still a live local and is closed before returning; fd_a is closed
  // by Dispose through its already-retained global reference.
  if (!MakeGlobal(env, fd_a.get(), &producer_fd_)) {
    CloseLocalDescriptors(env, os_class.get(), close, fd_a.get(), fd_b.get());
    return false;
  }
  if (!MakeGlobal(env, fd_b.get(), &receiver_fd_)) {
    CloseLocalDescriptors(env, os_class.get(), close, nullptr, fd_b.get());
    Dispose(env);
    return false;
  }

  LocalClass binder_class(env, env->FindClass("android/os/Binder"));
  jmethodID binder_ctor = binder_class.get() == nullptr || env->ExceptionCheck()
                              ? nullptr
                              : env->GetMethodID(binder_class.get(), "<init>",
                                                 "()V");
  LocalRef token(env, binder_ctor == nullptr || env->ExceptionCheck()
                           ? nullptr
                           : env->NewObject(binder_class.get(), binder_ctor));
  if (token.get() == nullptr || env->ExceptionCheck() ||
      !MakeGlobal(env, token.get(), &token_)) {
    Dispose(env);
    return false;
  }

  LocalClass parcel_class(env, env->FindClass("android/os/Parcel"));
  jmethodID obtain = parcel_class.get() == nullptr || env->ExceptionCheck()
                         ? nullptr
                         : env->GetStaticMethodID(parcel_class.get(), "obtain",
                                                  "()Landroid/os/Parcel;");
  jmethodID recycle = parcel_class.get() == nullptr || env->ExceptionCheck()
                          ? nullptr
                          : env->GetMethodID(parcel_class.get(), "recycle", "()V");
  jmethodID write_int = parcel_class.get() == nullptr || env->ExceptionCheck()
                            ? nullptr
                            : env->GetMethodID(parcel_class.get(), "writeInt",
                                               "(I)V");
  jmethodID write_binder =
      parcel_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(parcel_class.get(), "writeStrongBinder",
                             "(Landroid/os/IBinder;)V");
  jmethodID write_string =
      parcel_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(parcel_class.get(), "writeString",
                             "(Ljava/lang/String;)V");
  jmethodID write_fd =
      parcel_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(parcel_class.get(), "writeFileDescriptor",
                             "(Ljava/io/FileDescriptor;)V");
  jmethodID set_position =
      parcel_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(parcel_class.get(), "setDataPosition", "(I)V");
  if (obtain == nullptr || recycle == nullptr || write_int == nullptr ||
      write_binder == nullptr || write_string == nullptr || write_fd == nullptr ||
      set_position == nullptr || env->ExceptionCheck()) {
    Dispose(env);
    return false;
  }

  LocalRef parcel(env, env->CallStaticObjectMethod(parcel_class.get(), obtain));
  if (parcel.get() == nullptr || env->ExceptionCheck()) {
    Dispose(env);
    return false;
  }
  bool parcel_recycled = false;
  auto recycle_parcel = [&]() {
    if (parcel_recycled) return;
    parcel_recycled = true;
    ExceptionPreserver exceptions(env);
    env->CallVoidMethod(parcel.get(), recycle);
    exceptions.Capture();
  };

  env->CallVoidMethod(parcel.get(), write_int, 1);
  if (!env->ExceptionCheck())
    env->CallVoidMethod(parcel.get(), write_binder, token.get());
  LocalRef name(env, !env->ExceptionCheck() ? env->NewStringUTF(channel_name)
                                             : nullptr);
  if (name.get() != nullptr && !env->ExceptionCheck())
    env->CallVoidMethod(parcel.get(), write_string, name.get());
  if (!env->ExceptionCheck())
    env->CallVoidMethod(parcel.get(), write_fd, fd_b.get());
  if (!env->ExceptionCheck())
    env->CallVoidMethod(parcel.get(), set_position, 0);
  if (env->ExceptionCheck()) {
    recycle_parcel();
    Dispose(env);
    return false;
  }

  LocalClass channel_class(env, env->FindClass("android/view/InputChannel"));
  jmethodID channel_ctor = channel_class.get() == nullptr || env->ExceptionCheck()
                               ? nullptr
                               : env->GetMethodID(channel_class.get(), "<init>",
                                                  "()V");
  jmethodID read_from_parcel =
      channel_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(channel_class.get(), "readFromParcel",
                             "(Landroid/os/Parcel;)V");
  LocalRef channel(env, channel_ctor == nullptr || env->ExceptionCheck()
                             ? nullptr
                             : env->NewObject(channel_class.get(), channel_ctor));
  if (channel.get() == nullptr || read_from_parcel == nullptr ||
      env->ExceptionCheck()) {
    recycle_parcel();
    Dispose(env);
    return false;
  }
  env->CallVoidMethod(channel.get(), read_from_parcel, parcel.get());
  recycle_parcel();
  if (env->ExceptionCheck()) {
    DisposeLocalObject(env, channel.get(), "dispose");
    Dispose(env);
    return false;
  }
  if (!MakeGlobal(env, channel.get(), &channel_)) {
    DisposeLocalObject(env, channel.get(), "dispose");
    Dispose(env);
    return false;
  }
  if (!MakeGlobal(env, view_root, &view_root_)) {
    Dispose(env);
    return false;
  }

  LocalClass receiver_class(
      env, env->FindClass("android/view/ViewRootImpl$WindowInputEventReceiver"));
  jmethodID receiver_ctor =
      receiver_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(
                receiver_class.get(), "<init>",
                "(Landroid/view/ViewRootImpl;Landroid/view/InputChannel;Landroid/os/Looper;)V");
  LocalRef receiver(
      env, receiver_ctor == nullptr || env->ExceptionCheck()
               ? nullptr
               : env->NewObject(receiver_class.get(), receiver_ctor, view_root_,
                                channel_, owner_looper));
  if (receiver.get() == nullptr || env->ExceptionCheck()) {
    DisposeLocalObject(env, receiver.get(), "dispose");
    Dispose(env);
    return false;
  }
  if (!MakeGlobal(env, receiver.get(), &receiver_)) {
    DisposeLocalObject(env, receiver.get(), "dispose");
    Dispose(env);
    return false;
  }
  return true;
}

void FixtureInputChannel::Dispose(JNIEnv* env) {
  if (env == nullptr) return;
  if (owner_thread_ != std::thread::id() &&
      owner_thread_ != std::this_thread::get_id()) {
    ThrowNew(env, "java/lang/IllegalStateException",
             "fixture InputChannel must be disposed on its owner thread");
    return;
  }
  ExceptionPreserver exceptions(env);

  if (receiver_ != nullptr && !receiver_disposed_) {
    LocalClass receiver_class(env, env->GetObjectClass(receiver_));
    jmethodID dispose = receiver_class.get() == nullptr || env->ExceptionCheck()
                            ? nullptr
                            : env->GetMethodID(receiver_class.get(), "dispose",
                                               "()V");
    if (dispose != nullptr && !env->ExceptionCheck()) {
      receiver_disposed_ = true;
      env->CallVoidMethod(receiver_, dispose);
      exceptions.Capture();
    } else {
      exceptions.Capture();
    }
  }

  // InputEventReceiver's dispose is not relied on to close the separately
  // retained InputChannel object. Dispose it explicitly and independently.
  if (channel_ != nullptr && !channel_disposed_) {
    LocalClass channel_class(env, env->GetObjectClass(channel_));
    jmethodID dispose = channel_class.get() == nullptr || env->ExceptionCheck()
                            ? nullptr
                            : env->GetMethodID(channel_class.get(), "dispose",
                                               "()V");
    if (dispose != nullptr && !env->ExceptionCheck()) {
      channel_disposed_ = true;
      env->CallVoidMethod(channel_, dispose);
      exceptions.Capture();
    } else {
      exceptions.Capture();
    }
  }

  LocalClass os_class(env, env->FindClass("android/system/Os"));
  jmethodID close = os_class.get() == nullptr || env->ExceptionCheck()
                        ? nullptr
                        : env->GetStaticMethodID(
                              os_class.get(), "close",
                              "(Ljava/io/FileDescriptor;)V");
  if (close == nullptr || env->ExceptionCheck()) {
    exceptions.Capture();
  } else {
    if (producer_fd_ != nullptr && !producer_fd_closed_) {
      producer_fd_closed_ = true;
      env->CallStaticVoidMethod(os_class.get(), close, producer_fd_);
      exceptions.Capture();
    }
    if (receiver_fd_ != nullptr && !receiver_fd_closed_) {
      receiver_fd_closed_ = true;
      env->CallStaticVoidMethod(os_class.get(), close, receiver_fd_);
      exceptions.Capture();
    }
  }

  // Failed method lookup retains the corresponding global for a later
  // owner-thread retry. A call was attempted exactly once before release.
  if (receiver_ != nullptr && receiver_disposed_) {
    env->DeleteGlobalRef(receiver_);
    receiver_ = nullptr;
  }
  if (channel_ != nullptr && channel_disposed_) {
    env->DeleteGlobalRef(channel_);
    channel_ = nullptr;
  }
  if (view_root_ != nullptr && receiver_ == nullptr) {
    env->DeleteGlobalRef(view_root_);
    view_root_ = nullptr;
  }
  if (token_ != nullptr && channel_ == nullptr && receiver_ == nullptr) {
    env->DeleteGlobalRef(token_);
    token_ = nullptr;
  }
  if (producer_fd_ != nullptr && producer_fd_closed_) {
    env->DeleteGlobalRef(producer_fd_);
    producer_fd_ = nullptr;
  }
  if (receiver_fd_ != nullptr && receiver_fd_closed_) {
    env->DeleteGlobalRef(receiver_fd_);
    receiver_fd_ = nullptr;
  }
}

}  // namespace darwin_art_graphics_fixture
