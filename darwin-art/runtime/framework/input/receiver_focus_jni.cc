#include "receiver_focus_jni.h"
#include "receiver_jni_resources.h"

namespace darwin_art::input {
namespace {
template <typename T> class LocalRef final {
 public:
  LocalRef(JNIEnv* env, T value) : env_(env), value_(value) {}
  ~LocalRef() { if (value_ != nullptr) env_->DeleteLocalRef(value_); }
  T Get() const { return value_; }
  LocalRef(const LocalRef&) = delete;
  LocalRef& operator=(const LocalRef&) = delete;
 private:
  JNIEnv* env_;
  T value_;
};
}

ReceiverFocusNotificationResult NotifyInputReceiverFocus(
    JNIEnv* env, const std::shared_ptr<InputReceiver>& receiver, bool focused) {
  ReceiverFocusNotificationResult result;
  if (env == nullptr || receiver == nullptr || env->ExceptionCheck()) return result;
  ReceiverCallbackAdmission admission(env, receiver);
  if (!admission.Admitted() || receiver->disposed.load(std::memory_order_acquire) ||
      receiver->weak_receiver == nullptr) return result;
  LocalRef<jclass> weak_class(env, env->GetObjectClass(receiver->weak_receiver));
  if (weak_class.Get() == nullptr || env->ExceptionCheck()) return result;
  const auto get = env->GetMethodID(weak_class.Get(), "get", "()Ljava/lang/Object;");
  if (get == nullptr || env->ExceptionCheck()) return result;
  LocalRef<jobject> target(env, env->CallObjectMethod(receiver->weak_receiver, get));
  if (target.Get() == nullptr || env->ExceptionCheck() ||
      receiver->disposed.load(std::memory_order_acquire)) return result;
  LocalRef<jclass> type(env, env->FindClass("android/view/InputEventReceiver"));
  if (type.Get() == nullptr || env->ExceptionCheck()) return result;
  const auto method = env->GetMethodID(type.Get(), "onFocusEvent", "(Z)V");
  if (method == nullptr || env->ExceptionCheck() ||
      receiver->disposed.load(std::memory_order_acquire)) return result;
  result.invoked = true;
  env->CallVoidMethod(target.Get(), method, focused ? JNI_TRUE : JNI_FALSE);
  result.delivered = !env->ExceptionCheck() &&
                    !receiver->disposed.load(std::memory_order_acquire);
  return result;
}
}  // namespace darwin_art::input
