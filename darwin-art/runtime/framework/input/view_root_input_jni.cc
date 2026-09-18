#include "view_root_input_jni.h"

#include "receiver_admission.h"
#include "receiver_jni_resources.h"
#include "receiver_routing_access.h"

#include <new>
#include <utility>

namespace {

template <typename T>
class ScopedLocalRef final {
 public:
  ScopedLocalRef(JNIEnv* env, T value) : env_(env), value_(value) {}
  ~ScopedLocalRef() {
    if (env_ != nullptr && value_ != nullptr) env_->DeleteLocalRef(value_);
  }

  ScopedLocalRef(const ScopedLocalRef&) = delete;
  ScopedLocalRef& operator=(const ScopedLocalRef&) = delete;

  T get() const { return value_; }
  operator T() const { return value_; }
  T release() {
    T released = value_;
    value_ = nullptr;
    return released;
  }

 private:
  JNIEnv* env_;
  T value_;
};

using LocalClass = ScopedLocalRef<jclass>;
using LocalObject = ScopedLocalRef<jobject>;

// Resolve the weak WindowInputEventReceiver referent while preserving the
// exact Java exception state. The returned local reference belongs to caller.
LocalObject GetWeakReceiverTarget(JNIEnv* env,
                                  jobject weak_receiver) {
  if (env == nullptr || weak_receiver == nullptr ||
      env->ExceptionCheck()) {
    return LocalObject(env, nullptr);
  }
  LocalClass weak_class(env, env->GetObjectClass(weak_receiver));
  jmethodID weak_get = weak_class.get() == nullptr || env->ExceptionCheck()
                           ? nullptr
                           : env->GetMethodID(weak_class.get(), "get",
                                              "()Ljava/lang/Object;");
  if (weak_get == nullptr || env->ExceptionCheck()) {
    return LocalObject(env, nullptr);
  }
  return LocalObject(env, env->CallObjectMethod(weak_receiver,
                                                weak_get));
}

LocalObject GetWeakReceiverTarget(JNIEnv* env,
                                  darwin_art::input::InputReceiver* receiver) {
  return GetWeakReceiverTarget(env, receiver == nullptr ? nullptr : receiver->weak_receiver);
}

bool IsExactOriginalReceiverCurrent(
    const darwin_art::input::OriginalInputReceiverDispatchContext& context,
    const darwin_art::input::InputRoutingRecipientHandle& expected,
    const darwin_art::input::InputRoutingHandle& routing) {
  const auto& receiver = context.receiver;
  if (receiver == nullptr || expected == nullptr || routing == nullptr ||
      receiver->disposed.load(std::memory_order_acquire) ||
      receiver->registry_id == 0 ||
      darwin_art::input::ReceiverRoutingHandle(receiver.get()) != routing ||
      receiver->routing_recipient.lock() != expected ||
      expected->Routing() != routing)
    return false;
  // The token identity and numeric ID are both checked. IDs are opaque and
  // never reused, while the token check prevents a stale admitted callback
  // from crossing an exact replacement on the same channel.
  return darwin_art::input::InputRoutingConsumerMatches(
      routing, receiver->registry_id);
}

}  // namespace

namespace darwin_art::input {

ReceiverViewRootResolution ResolveWeakWindowReceiverViewRoot(JNIEnv* env, jobject weak_receiver) {
  if (env == nullptr || env->ExceptionCheck()) return {};
  if (weak_receiver == nullptr) return {ReceiverViewRootKind::kAbsent, nullptr};
  LocalObject target = GetWeakReceiverTarget(env, weak_receiver);
  if (env->ExceptionCheck()) return {};
  if (target.get() == nullptr) return {ReceiverViewRootKind::kAbsent, nullptr};
  LocalClass type(env, env->FindClass("android/view/ViewRootImpl$WindowInputEventReceiver"));
  if (type.get() == nullptr || env->ExceptionCheck()) return {};
  const bool is_window = env->IsInstanceOf(target.get(), type.get()) == JNI_TRUE;
  if (env->ExceptionCheck()) return {};
  if (!is_window) return {ReceiverViewRootKind::kGeneral, nullptr};
  jfieldID field = env->GetFieldID(type.get(), "this$0", "Landroid/view/ViewRootImpl;");
  if (field == nullptr || env->ExceptionCheck()) return {};
  LocalObject root(env, env->GetObjectField(target.get(), field));
  if (env->ExceptionCheck() || root.get() == nullptr) return {};
  return {ReceiverViewRootKind::kWindow, root.release()};
}

bool ReadViewRootGeometry(JNIEnv* env, jobject view_root,
                          InputWindowGeometry* geometry) {
  if (env == nullptr || view_root == nullptr || geometry == nullptr ||
      env->ExceptionCheck()) {
    return false;
  }
  LocalClass root_class(env, env->GetObjectClass(view_root));
  jfieldID frame_field =
      root_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetFieldID(root_class.get(), "mWinFrame",
                            "Landroid/graphics/Rect;");
  LocalObject frame(env, frame_field == nullptr || env->ExceptionCheck()
                             ? nullptr
                             : env->GetObjectField(view_root, frame_field));
  LocalClass rect_class(
      env, frame.get() == nullptr || env->ExceptionCheck()
               ? nullptr
               : env->GetObjectClass(frame.get()));
  jfieldID left_field =
      rect_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetFieldID(rect_class.get(), "left", "I");
  jfieldID top_field =
      rect_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetFieldID(rect_class.get(), "top", "I");
  jfieldID right_field =
      rect_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetFieldID(rect_class.get(), "right", "I");
  jfieldID bottom_field =
      rect_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetFieldID(rect_class.get(), "bottom", "I");
  if (frame.get() == nullptr || left_field == nullptr || top_field == nullptr ||
      right_field == nullptr || bottom_field == nullptr || env->ExceptionCheck()) {
    return false;
  }
  InputWindowGeometry read{
      env->GetIntField(frame.get(), left_field),
      env->GetIntField(frame.get(), top_field),
      env->GetIntField(frame.get(), right_field),
      env->GetIntField(frame.get(), bottom_field),
  };
  if (env->ExceptionCheck()) return false;
  *geometry = read;
  return true;
}

FrameworkInputEventDispatchResult DispatchFrameworkInputEventResultForReceiver(
    JNIEnv* env, const OriginalInputReceiverDispatchContext& context,
    jobject event) {
  FrameworkInputEventDispatchResult result;
  const auto& receiver = context.receiver;
  if (env == nullptr || receiver == nullptr || event == nullptr ||
      env->ExceptionCheck() ||
      receiver->weak_receiver == nullptr ||
      receiver->disposed.load(std::memory_order_acquire))
    return result;

  ReceiverCallbackAdmission admission(env, receiver);
  if (!admission.Admitted()) return result;
  // The caller pins the immutable publication token before packet/JNI work.
  // Keep using that strong identity; reacquiring the receiver's weak token
  // here would allow a reentrant replacement to alter the dispatch target.
  const auto& expected = context.recipient;
  const auto routing = ReceiverRoutingHandle(receiver.get());
  if (!IsExactOriginalReceiverCurrent(context, expected, routing)) return result;

  LocalObject target = GetWeakReceiverTarget(env, receiver.get());
  if (target.get() == nullptr || env->ExceptionCheck() ||
      !IsExactOriginalReceiverCurrent(context, expected, routing))
    return result;
  LocalClass view_root_class(
      env, receiver->view_root == nullptr ? nullptr
                                         : env->GetObjectClass(receiver->view_root));
  LocalClass input_receiver_class(
      env, env->ExceptionCheck()
               ? nullptr
               : env->FindClass("android/view/InputEventReceiver"));
  LocalClass motion_event_class(
      env, env->ExceptionCheck() ? nullptr
                                 : env->FindClass("android/view/MotionEvent"));
  const bool is_motion = motion_event_class.get() != nullptr &&
                         !env->ExceptionCheck() &&
                         env->IsInstanceOf(event, motion_event_class.get());
  jmethodID get_source =
      motion_event_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(motion_event_class.get(), "getSource", "()I");
  const jint source =
      is_motion && get_source != nullptr && !env->ExceptionCheck()
          ? env->CallIntMethod(event, get_source)
          : 0;
  const bool is_touchscreen = source == 0x1002;
  if (is_touchscreen && !receiver->touch_mode &&
      input_receiver_class.get() != nullptr && !env->ExceptionCheck()) {
    jmethodID touch_mode = env->GetMethodID(
        input_receiver_class.get(), "onTouchModeChanged", "(Z)V");
    jmethodID handle_touch_mode =
        view_root_class.get() == nullptr || env->ExceptionCheck()
            ? nullptr
            : env->GetMethodID(view_root_class.get(),
                               "handleWindowTouchModeChanged", "()V");
    if (touch_mode != nullptr && handle_touch_mode != nullptr &&
        !env->ExceptionCheck()) {
      env->CallVoidMethod(target.get(), touch_mode, JNI_TRUE);
      if (!IsExactOriginalReceiverCurrent(context, expected, routing) ||
          env->ExceptionCheck())
        return result;
      env->CallVoidMethod(receiver->view_root, handle_touch_mode);
      if (!IsExactOriginalReceiverCurrent(context, expected, routing) ||
          env->ExceptionCheck())
        return result;
      receiver->touch_mode = true;
    }
  }
  if (!IsExactOriginalReceiverCurrent(context, expected, routing) ||
      env->ExceptionCheck())
    return result;
  jmethodID dispatch =
      input_receiver_class.get() == nullptr || env->ExceptionCheck()
          ? nullptr
          : env->GetMethodID(input_receiver_class.get(), "dispatchInputEvent",
                             "(ILandroid/view/InputEvent;)V");
  if (target.get() == nullptr || dispatch == nullptr || env->ExceptionCheck())
    return result;

  // AllocateReceiverFinish is intentionally before the final exact-token
  // check: endpoint bookkeeping may synchronously reenter the owner. The
  // check directly below is the last operation before dispatchInputEvent.
  jint sequence = 0;
  if (!AllocateReceiverFinish(receiver.get(), &sequence, context.origin,
                              context.recipient))
    return result;  // Capacity is reserved before Android takes the event.
  if (!IsExactOriginalReceiverCurrent(context, expected, routing) ||
      env->ExceptionCheck()) {
    (void)CancelReceiverFinish(receiver.get(), sequence);
    return result;
  }
  result.framework_sequence = sequence;
  if (context.local_packet_lease &&
      !BeginInputRoutingPacketDelivery(*context.local_packet_lease,
                                      context.recipient, routing)) {
    // Authority rejection is terminal, not retryable backpressure. All input
    // locks have been released before finish-ledger cleanup can reenter.
    (void)CancelReceiverFinish(receiver.get(), sequence);
    result.rejected = true;
    return result;
  }
  result.invoked = true;
  env->CallVoidMethod(target.get(), dispatch, sequence, event);
  bool finished_handled = false;
  result.acknowledged = CloseReceiverFinishObservation(
      receiver.get(), sequence, &finished_handled);
  result.handled = finished_handled;
  result.delivered = result.invoked && !env->ExceptionCheck();
  return result;
}

}  // namespace darwin_art::input
