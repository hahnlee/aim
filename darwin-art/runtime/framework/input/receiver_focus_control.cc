#include "receiver_focus_control.h"
#include "receiver_focus_jni.h"
#include "receiver_jni_resources.h"
#include "input_channel_jni_resources.h"
#include "input_routing_focus.h"
#include <new>

namespace darwin_art::input {
namespace {
void ReportFailure(JNIEnv* env) noexcept {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass type = env->FindClass("java/lang/RuntimeException");
  if (type != nullptr) {
    (void)env->ThrowNew(type, "Native input focus transaction failed");
    env->DeleteLocalRef(type);
  }
}
}
FocusControlCallbackResult ConsumeReceiverFocusControl(
    JNIEnv* env, const std::shared_ptr<InputReceiver>& receiver,
    const InputRoutingRecipientHandle& expected, FocusControl control) noexcept {
  if (env == nullptr || env->ExceptionCheck())
    return FocusControlCallbackResult::kDeferred;
  if (receiver == nullptr || expected == nullptr ||
      receiver->disposed.load(std::memory_order_acquire) ||
      receiver->routing_recipient.lock() != expected)
    return FocusControlCallbackResult::kConsumedStop;
  bool invoked = false;
  try {
    auto result = ApplyInputRoutingFocusControl(expected, control);
    if (result.decision == InputFocusEpochEvaluator::Decision::kDeferredPendingLocalInput)
      return FocusControlCallbackResult::kDeferred;
    if (!result.Accepted()) return FocusControlCallbackResult::kConsumed;
    // A previous gain may have committed before JNI could resolve its target.
    // Retry only the owner's authoritative cached grant, never caller state.
    if (!result.ShouldNotify() && control.focused &&
        result.decision == InputFocusEpochEvaluator::Decision::kDuplicate)
      result = ReplayCachedInputRoutingFocus(expected);
    if (result.decision == InputFocusEpochEvaluator::Decision::kDeferredPendingLocalInput)
      return FocusControlCallbackResult::kDeferred;
    if (!result.ShouldNotify()) return FocusControlCallbackResult::kConsumed;
    const auto notification = NotifyInputReceiverFocus(env, receiver, result.control.focused);
    invoked = notification.invoked;
    (void)CommitInputRoutingFocusNotification(result, notification.invoked);
    (void)CommitInputRoutingFocusReadiness(result, notification.delivered);
    if (!notification.invoked) {
      // A recoverable pre-invocation failure has not consumed the FIFO
      // obligation. Cache commit is not Java delivery (gain or loss).
      return receiver->disposed.load(std::memory_order_acquire) ||
                     receiver->routing_recipient.lock() != expected
                 ? FocusControlCallbackResult::kConsumedStop
                 : FocusControlCallbackResult::kDeferred;
    }
    if (env->ExceptionCheck() ||
        receiver->disposed.load(std::memory_order_acquire))
      return FocusControlCallbackResult::kConsumedStop;
    return FocusControlCallbackResult::kConsumed;
  } catch (const std::bad_alloc&) {
    ThrowInputChannelOutOfMemory(env);
  } catch (...) {
    ReportFailure(env);
  }
  if (invoked) return FocusControlCallbackResult::kConsumedStop;
  // A failed ledger preparation has not admitted this control. Preserve both
  // its FIFO head and the Java error; do not synthesize successful focus.
  return FocusControlCallbackResult::kDeferred;
}
}  // namespace darwin_art::input
