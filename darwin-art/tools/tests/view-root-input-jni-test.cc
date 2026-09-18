#include "runtime/framework/input/view_root_input_jni.h"
#include "runtime/framework/input/receiver_focus_jni.h"

#include "runtime/framework/input/packet_dispatch.h"
#include "runtime/framework/input/receiver_jni_resources.h"
#include "runtime/framework/input/receiver_registry.h"
#include "runtime/framework/input/receiver_routing_access.h"

#include <cassert>
#include <cstdint>
#include <cstring>
#include <cstdarg>
#include <memory>
#include <limits>

namespace {

bool g_exception = false;
bool g_window_receiver = false;
bool g_root_lookup_failure = false;
bool g_cleared_weak = false;
int g_root_lookups = 0;
int g_receiver_identity_lookups = 0;
bool g_dispose_on_callback = false;
int g_dispatch_calls = 0;
int g_handle_touch_mode_calls = 0;
int g_touch_mode_calls = 0;
int g_global_deletes = 0;
int g_progress_requests = 0;
int g_registered_finish = 0;
int g_canceled_finish = 0;
int g_delivery_admissions = 0;
bool g_finish_full = false;
int g_finish_sequence = 0;
darwin_art::input::InputEventOrigin g_reserved_origin;
int g_focus_calls = 0;
int g_local_deletes = 0;
int g_global_creates = 0;
bool g_focus_value = false;
bool g_throw_on_focus = false;
bool g_throw_on_dispatch = false;
bool g_fail_focus_lookup = false;
bool g_dispose_on_weak_get = false;
bool g_replace_on_weak_get = false;
bool g_replace_on_touch_mode = false;
bool g_replace_on_handle_touch_mode = false;
bool g_replace_on_finish = false;
std::shared_ptr<darwin_art::input::InputReceiver> g_receiver;
std::shared_ptr<const darwin_art::input::InputRoutingRecipient> g_replacement;
darwin_art::input::InputRoutingHandle g_test_routing;

enum : uintptr_t {
  kRootClass = 0x101,
  kReceiverClass = 0x102,
  kRectClass = 0x103,
  kInputReceiverClass = 0x104,
  kMotionEventClass = 0x105,
  kRoot = 0x201,
  kJavaReceiver = 0x202,
  kFrame = 0x203,
  kTarget = 0x204,
  kEvent = 0x205,
  kReceiverField = 0x301,
  kPointerField = 0x302,
  kFrameField = 0x303,
  kLeftField = 0x304,
  kTopField = 0x305,
  kRightField = 0x306,
  kBottomField = 0x307,
  kWeakGet = 0x401,
  kFocus = 0x402,
  kTouchMode = 0x403,
  kHandleTouchMode = 0x404,
  kDispatch = 0x405,
  kGetSource = 0x406,
};

template <typename T>
T Handle(uintptr_t value) {
  return reinterpret_cast<T>(value);
}

jclass GetObjectClass(JNIEnv*, jobject object) {
  switch (reinterpret_cast<uintptr_t>(object)) {
    case kRoot: return Handle<jclass>(kRootClass);
    case kJavaReceiver: return Handle<jclass>(kReceiverClass);
    case kFrame: return Handle<jclass>(kRectClass);
    default: return Handle<jclass>(kReceiverClass);
  }
}

jfieldID GetFieldID(JNIEnv*, jclass klass, const char* name, const char*) {
  if (std::strcmp(name, "this$0") == 0) {
    ++g_root_lookups;
    if (g_root_lookup_failure) { g_exception = true; return nullptr; }
    return Handle<jfieldID>(0x308);
  }
  if (klass == Handle<jclass>(kRootClass) &&
      std::strcmp(name, "mInputEventReceiver") == 0) {
    ++g_receiver_identity_lookups;
    return Handle<jfieldID>(kReceiverField);
  }
  if (klass == Handle<jclass>(kReceiverClass) &&
      std::strcmp(name, "mReceiverPtr") == 0) {
    ++g_receiver_identity_lookups;
    return Handle<jfieldID>(kPointerField);
  }
  if (klass == Handle<jclass>(kRootClass) &&
      std::strcmp(name, "mWinFrame") == 0)
    return Handle<jfieldID>(kFrameField);
  if (klass == Handle<jclass>(kRectClass)) {
    if (std::strcmp(name, "left") == 0) return Handle<jfieldID>(kLeftField);
    if (std::strcmp(name, "top") == 0) return Handle<jfieldID>(kTopField);
    if (std::strcmp(name, "right") == 0) return Handle<jfieldID>(kRightField);
    if (std::strcmp(name, "bottom") == 0) return Handle<jfieldID>(kBottomField);
  }
  return nullptr;
}

jobject GetObjectField(JNIEnv*, jobject object, jfieldID field) {
  if (field == Handle<jfieldID>(0x308)) return Handle<jobject>(kRoot);
  if (object == Handle<jobject>(kRoot) && field == Handle<jfieldID>(kReceiverField))
    return Handle<jobject>(kJavaReceiver);
  if (object == Handle<jobject>(kRoot) && field == Handle<jfieldID>(kFrameField))
    return Handle<jobject>(kFrame);
  return nullptr;
}

jlong GetLongField(JNIEnv*, jobject, jfieldID field) {
  return field == Handle<jfieldID>(kPointerField) ? 17 : 0;
}

jint GetIntField(JNIEnv*, jobject, jfieldID field) {
  if (field == Handle<jfieldID>(kLeftField)) return 1;
  if (field == Handle<jfieldID>(kTopField)) return 2;
  if (field == Handle<jfieldID>(kRightField)) return 301;
  return field == Handle<jfieldID>(kBottomField) ? 402 : 0;
}

jmethodID GetMethodID(JNIEnv*, jclass, const char* name, const char*) {
  if (std::strcmp(name, "get") == 0) return Handle<jmethodID>(kWeakGet);
  if (std::strcmp(name, "onFocusEvent") == 0) {
    if (g_fail_focus_lookup) { g_exception = true; return nullptr; }
    return Handle<jmethodID>(kFocus);
  }
  if (std::strcmp(name, "onTouchModeChanged") == 0)
    return Handle<jmethodID>(kTouchMode);
  if (std::strcmp(name, "handleWindowTouchModeChanged") == 0)
    return Handle<jmethodID>(kHandleTouchMode);
  if (std::strcmp(name, "dispatchInputEvent") == 0)
    return Handle<jmethodID>(kDispatch);
  return std::strcmp(name, "getSource") == 0 ? Handle<jmethodID>(kGetSource)
                                             : nullptr;
}

jobject CallObjectMethodV(JNIEnv* env, jobject, jmethodID, va_list) {
  if (g_cleared_weak) return nullptr;
  if (g_dispose_on_weak_get && g_receiver != nullptr) {
    darwin_art::input::RetireReceiverResources(env, g_receiver);
    assert(!g_receiver->refs_cleaned && g_global_deletes == 0);
    assert(g_receiver->weak_receiver != nullptr);
  }
  if (g_replace_on_weak_get && g_receiver != nullptr)
    g_receiver->routing_recipient = g_replacement;
  return Handle<jobject>(kTarget);
}

jint CallIntMethodV(JNIEnv*, jobject object, jmethodID, va_list) {
  return object == Handle<jobject>(kEvent) ? 0x1002 : 0;
}

void CallVoidMethodV(JNIEnv* env, jobject object, jmethodID method, va_list arguments) {
  if (method == Handle<jmethodID>(kFocus)) {
    assert(object == Handle<jobject>(kTarget));
    ++g_focus_calls;
    g_focus_value = va_arg(arguments, int) != 0;
    if (g_throw_on_focus) g_exception = true;
  }
  if (method == Handle<jmethodID>(kTouchMode)) ++g_touch_mode_calls;
  if (method == Handle<jmethodID>(kDispatch)) ++g_dispatch_calls;
  if (method == Handle<jmethodID>(kHandleTouchMode))
    ++g_handle_touch_mode_calls;
  if (g_dispose_on_callback &&
      (method == Handle<jmethodID>(kFocus) ||
       method == Handle<jmethodID>(kTouchMode) ||
       method == Handle<jmethodID>(kHandleTouchMode))) {
    (void)object;
    if (g_receiver != nullptr) {
      darwin_art::input::RetireReceiverResources(env, g_receiver);
      assert(!g_receiver->refs_cleaned && g_global_deletes == 0);
    }
  }
  if (g_replace_on_touch_mode && method == Handle<jmethodID>(kTouchMode) &&
      g_receiver != nullptr)
    g_receiver->routing_recipient = g_replacement;
  if (g_replace_on_handle_touch_mode && method == Handle<jmethodID>(kHandleTouchMode) &&
      g_receiver != nullptr)
    g_receiver->routing_recipient = g_replacement;
  if (g_throw_on_dispatch && method == Handle<jmethodID>(kDispatch))
    g_exception = true;
}

jclass FindClass(JNIEnv*, const char* name) {
  if (std::strcmp(name, "android/view/ViewRootImpl$WindowInputEventReceiver") == 0)
    return Handle<jclass>(kReceiverClass);
  if (std::strcmp(name, "android/view/InputEventReceiver") == 0)
    return Handle<jclass>(kInputReceiverClass);
  if (std::strcmp(name, "android/view/MotionEvent") == 0)
    return Handle<jclass>(kMotionEventClass);
  return nullptr;
}

jboolean IsInstanceOf(JNIEnv*, jobject object, jclass klass) {
  if (klass == Handle<jclass>(kReceiverClass)) return g_window_receiver ? JNI_TRUE : JNI_FALSE;
  return object == Handle<jobject>(kEvent) &&
                 klass == Handle<jclass>(kMotionEventClass)
             ? JNI_TRUE
             : JNI_FALSE;
}

jboolean ExceptionCheck(JNIEnv*) { return g_exception ? JNI_TRUE : JNI_FALSE; }
void DeleteLocalRef(JNIEnv*, jobject) { ++g_local_deletes; }
void DeleteGlobalRef(JNIEnv*, jobject) { ++g_global_deletes; }
jobject NewGlobalRef(JNIEnv*, jobject object) { ++g_global_creates; return object; }
jint ThrowNew(JNIEnv*, jclass, const char*) {
  g_exception = true;
  return JNI_OK;
}

}  // namespace

namespace darwin_art::input {

// Controlled routing admission port in this JNI-owner test. The real gate is
// exercised separately by root-key-routing-test; this proves call ordering
// and terminal rejection/finish cleanup, not authority policy.
bool BeginInputRoutingPacketDelivery(InputRoutingPacketLease&,
    const InputRoutingRecipientHandle& recipient, const InputRoutingHandle& routing) {
  assert(g_registered_finish == 1 && g_dispatch_calls == 0);
  assert(recipient == g_receiver->routing_recipient.lock() &&
         routing == ReceiverRoutingHandle(g_receiver.get()));
  ++g_delivery_admissions;
  return false;
}

ReceiverEndpointBinding::ReceiverEndpointBinding() = default;
ReceiverEndpointBinding::~ReceiverEndpointBinding() = default;

bool ReceiverEndpointBinding::Retire() { return true; }

std::shared_ptr<InputReceiver> AcquireInputReceiver(ReceiverId id) {
  return id == 17 ? g_receiver : nullptr;
}

InputRoutingHandle ReceiverRoutingHandle(const InputReceiver*) {
  static InputRoutingHandle routing(
      reinterpret_cast<InputRoutingState*>(0x701), [](InputRoutingState*) {});
  return routing;
}
bool AllocateReceiverFinish(InputReceiver* receiver, jint* sequence,
                            InputEventOrigin origin, InputRoutingRecipientHandle original) {
  ++g_registered_finish;
  if (g_finish_full) return false;
  const uint32_t allocated = receiver->next_sequence++;
  *sequence = allocated <= static_cast<uint32_t>(std::numeric_limits<jint>::max())
                  ? static_cast<jint>(allocated)
                  : static_cast<jint>(static_cast<int64_t>(allocated) - (int64_t{1} << 32));
  g_finish_sequence = *sequence;
  assert(original == g_receiver->routing_recipient.lock());
  g_reserved_origin = origin;
  if (g_replace_on_finish && g_receiver != nullptr)
    g_receiver->routing_recipient = g_replacement;
  return true;
}
bool CancelReceiverFinish(const InputReceiver*, jint sequence) {
  assert(sequence == g_finish_sequence);
  ++g_canceled_finish;
  g_finish_sequence = 0;
  return true;
}
bool CloseReceiverFinishObservation(const InputReceiver*, jint sequence, bool* handled) {
  if (handled != nullptr) *handled = sequence == g_finish_sequence;
  return sequence == g_finish_sequence;
}
void RequestReceiverRoutingProgress(const InputReceiver*) { ++g_progress_requests; }
bool UpdateInputRoutingReceiverGeometry(const InputRoutingHandle&, int32_t left,
                                int32_t top, int32_t right, int32_t bottom,
                                ReceiverId) {
  return left == 1 && top == 2 && right == 301 && bottom == 402;
}
bool InputRoutingConsumerMatches(const InputRoutingHandle&, ReceiverId id) {
  return id == 17;
}

}  // namespace darwin_art::input

namespace {

JNIEnv MakeEnv();
void ResetReceiver() {
  if (g_receiver != nullptr) {
    JNIEnv env = MakeEnv();
    darwin_art::input::RetireReceiverResources(&env, g_receiver);
    // A leaked real callback admission would prevent actual JNI cleanup.
    assert(g_receiver->refs_cleaned && g_receiver->admission.IsQuiescent());
  }
  g_exception = false;
  g_dispose_on_callback = false;
  g_dispatch_calls = 0;
  g_handle_touch_mode_calls = 0;
  g_touch_mode_calls = 0;
  g_global_deletes = 0;
  g_progress_requests = 0;
  g_registered_finish = 0;
  g_canceled_finish = 0;
  g_delivery_admissions = 0;
  g_finish_full = false;
  g_finish_sequence = 0;
  g_focus_calls = 0;
  g_local_deletes = 0;
  g_global_creates = 0;
  g_throw_on_focus = false;
  g_throw_on_dispatch = false;
  g_fail_focus_lookup = false;
  g_dispose_on_weak_get = false;
  g_replace_on_weak_get = false;
  g_replace_on_touch_mode = false;
  g_replace_on_handle_touch_mode = false;
  g_replace_on_finish = false;
  g_replacement.reset();
  g_test_routing.reset();
  g_receiver = std::make_shared<darwin_art::input::InputReceiver>();
  g_receiver->registry_id = 17;
  g_receiver->weak_receiver = Handle<jobject>(kJavaReceiver);
}

JNIEnv MakeEnv() {
  static JNINativeInterface table{};
  table.GetObjectClass = GetObjectClass;
  table.GetFieldID = GetFieldID;
  table.GetObjectField = GetObjectField;
  table.GetLongField = GetLongField;
  table.GetIntField = GetIntField;
  table.GetMethodID = GetMethodID;
  table.CallObjectMethodV = CallObjectMethodV;
  table.CallIntMethodV = CallIntMethodV;
  table.CallVoidMethodV = CallVoidMethodV;
  table.FindClass = FindClass;
  table.IsInstanceOf = IsInstanceOf;
  table.ExceptionCheck = ExceptionCheck;
  table.DeleteLocalRef = DeleteLocalRef;
  table.DeleteGlobalRef = DeleteGlobalRef;
  table.NewGlobalRef = NewGlobalRef;
  table.ThrowNew = ThrowNew;
  return JNIEnv{&table};
}

}  // namespace

int main() {
  JNIEnv env = MakeEnv();
  using darwin_art::input::ReceiverViewRootKind;
  using darwin_art::input::ResolveWeakWindowReceiverViewRoot;
  auto weak = Handle<jobject>(kJavaReceiver);
  auto resolved = ResolveWeakWindowReceiverViewRoot(&env, weak);
  assert(resolved.kind == ReceiverViewRootKind::kGeneral && g_root_lookups == 0);
  g_window_receiver = true;
  resolved = ResolveWeakWindowReceiverViewRoot(&env, weak);
  assert(resolved.kind == ReceiverViewRootKind::kWindow && resolved.local_root == Handle<jobject>(kRoot));
  env.DeleteLocalRef(resolved.local_root);
  g_root_lookup_failure = true;
  resolved = ResolveWeakWindowReceiverViewRoot(&env, weak);
  assert(resolved.kind == ReceiverViewRootKind::kError && g_exception);
  const int lookups_before = g_root_lookups;
  assert(ResolveWeakWindowReceiverViewRoot(&env, weak).kind == ReceiverViewRootKind::kError);
  assert(g_exception && g_root_lookups == lookups_before);
  g_exception = false; g_root_lookup_failure = false; g_cleared_weak = true;
  assert(ResolveWeakWindowReceiverViewRoot(&env, weak).kind == ReceiverViewRootKind::kAbsent);
  g_cleared_weak = false;
  ResetReceiver();
  const auto gain = darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true);
  assert(gain.invoked && gain.delivered && g_focus_calls == 1 && g_focus_value);
  assert(g_local_deletes == 3 && g_global_creates == 0);
  const auto loss = darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, false);
  assert(loss.invoked && loss.delivered && g_focus_calls == 2 && !g_focus_value);
  assert(g_receiver->view_root == nullptr && g_progress_requests == 0);

  ResetReceiver();
  g_exception = true;
  assert(!darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true).invoked);
  assert(g_exception && g_local_deletes == 0);

  ResetReceiver();
  g_cleared_weak = true;
  assert(!darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true).invoked);
  assert(g_local_deletes == 1);
  g_cleared_weak = false;

  ResetReceiver();
  g_receiver->disposed.store(true, std::memory_order_release);
  assert(!darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true).invoked);
  assert(g_focus_calls == 0 && g_local_deletes == 0);

  ResetReceiver();
  g_dispose_on_weak_get = true;
  assert(!darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true).invoked);
  assert(g_focus_calls == 0 && g_local_deletes == 2);
  assert(g_receiver->refs_cleaned && g_global_deletes == 1);

  ResetReceiver();
  g_fail_focus_lookup = true;
  assert(!darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true).invoked);
  assert(g_exception && g_focus_calls == 0 && g_local_deletes == 3);

  ResetReceiver();
  g_throw_on_focus = true;
  const auto throwing = darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true);
  assert(throwing.invoked && !throwing.delivered && g_exception && g_focus_calls == 1);
  assert(g_local_deletes == 3);

  ResetReceiver();
  g_dispose_on_callback = true;
  const auto retiring = darwin_art::input::NotifyInputReceiverFocus(&env, g_receiver, true);
  assert(retiring.invoked && !retiring.delivered && g_focus_calls == 1);
  assert(g_local_deletes == 3 && g_global_creates == 0);
  assert(g_receiver->refs_cleaned && g_global_deletes == 1);

  const auto make_original_context = []() {
    g_receiver->view_root = Handle<jobject>(kRoot);
    auto routing = darwin_art::input::InputRoutingHandle(
        reinterpret_cast<darwin_art::input::InputRoutingState*>(0x701),
        [](darwin_art::input::InputRoutingState*) {});
    g_test_routing = routing;
    auto recipient = std::make_shared<const darwin_art::input::InputRoutingRecipient>(
        routing, 17, darwin_art::input::InputRoutingEndpointHandle{});
    g_receiver->routing_recipient = recipient;
    return darwin_art::input::OriginalInputReceiverDispatchContext{
        g_receiver, recipient, nullptr,
        {darwin_art::input::ReceiverPacketOrigin::kImportedChannel, (1ULL << 48) + 77}};
  };

  // The packet path supplies the admitted original publication token. It
  // must dispatch to that weak receiver without consulting the mutable
  // ViewRootImpl.mInputEventReceiver pointer/ID.
  ResetReceiver();
  const int identity_lookups_before_original = g_receiver_identity_lookups;
  auto context = make_original_context();
  const auto original =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(original.invoked && original.delivered && original.acknowledged &&
         original.handled && g_dispatch_calls == 1 &&
         g_receiver_identity_lookups == identity_lookups_before_original &&
         !g_receiver->disposed);
  assert(g_reserved_origin.kind == darwin_art::input::ReceiverPacketOrigin::kImportedChannel &&
         g_reserved_origin.sequence == (1ULL << 48) + 77);

  // InputEventReceiver also supports non-window consumers; a missing
  // ViewRoot must not make GetObjectClass(null) reject a valid key dispatch.
  ResetReceiver();
  context = make_original_context();
  g_receiver->view_root = nullptr;
  const auto general =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(general.invoked && general.delivered && g_dispatch_calls == 1 &&
         !g_exception && g_touch_mode_calls == 0);

  // A replacement during weak-get must stop before any framework handoff.
  ResetReceiver();
  context = make_original_context();
  g_replacement = std::make_shared<const darwin_art::input::InputRoutingRecipient>(
      context.recipient->Routing(), 18,
      darwin_art::input::InputRoutingEndpointHandle{});
  g_replace_on_weak_get = true;
  const auto weak_replaced =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(!weak_replaced.invoked && g_dispatch_calls == 0);

  ResetReceiver();
  context = make_original_context();
  g_dispose_on_weak_get = true;
  const auto weak_disposed =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(!weak_disposed.invoked && g_dispatch_calls == 0);
  assert(g_receiver->refs_cleaned && g_global_deletes == 2);

  // Preserve the former legacy dispatch test on the real typed product path:
  // touch-mode delivery retires the receiver before the event handoff.
  ResetReceiver();
  context = make_original_context();
  g_dispose_on_callback = true;
  const auto touch_disposed =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(kEvent));
  assert(!touch_disposed.invoked && g_dispatch_calls == 0 &&
         g_touch_mode_calls == 1 && g_handle_touch_mode_calls == 0);
  assert(g_receiver->refs_cleaned && g_global_deletes == 2);

  // Touch-mode JNI may retire/replace the route; validate again before the
  // dispatch method is looked up/handed the event.
  ResetReceiver();
  context = make_original_context();
  g_replacement = std::make_shared<const darwin_art::input::InputRoutingRecipient>(
      context.recipient->Routing(), 18,
      darwin_art::input::InputRoutingEndpointHandle{});
  g_replace_on_touch_mode = true;
  const auto touch_replaced =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(kEvent));
  assert(!touch_replaced.invoked && g_dispatch_calls == 0 &&
         g_touch_mode_calls == 1 && g_handle_touch_mode_calls == 0 &&
         !g_receiver->disposed);

  ResetReceiver();
  context = make_original_context();
  g_replacement = std::make_shared<const darwin_art::input::InputRoutingRecipient>(
      context.recipient->Routing(), 18,
      darwin_art::input::InputRoutingEndpointHandle{});
  g_replace_on_handle_touch_mode = true;
  const auto handle_replaced =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(kEvent));
  assert(!handle_replaced.invoked && g_dispatch_calls == 0 &&
         g_touch_mode_calls == 1 && g_handle_touch_mode_calls == 1 &&
         !g_receiver->disposed);

  // Finish bookkeeping is the last reentrant operation before the final
  // exact-token check; replacement there must also prevent dispatch.
  ResetReceiver();
  context = make_original_context();
  g_replacement = std::make_shared<const darwin_art::input::InputRoutingRecipient>(
      context.recipient->Routing(), 18,
      darwin_art::input::InputRoutingEndpointHandle{});
  g_replace_on_finish = true;
  const auto finish_replaced =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(!finish_replaced.invoked && g_dispatch_calls == 0 &&
         g_registered_finish == 1 && g_canceled_finish == 1);

  ResetReceiver();
  context = make_original_context();
  // The controlled port never dereferences this borrowed identity sentinel.
  context.local_packet_lease =
      reinterpret_cast<darwin_art::input::InputRoutingPacketLease*>(0x900);
  const auto rejected_authority =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(rejected_authority.rejected && !rejected_authority.invoked &&
         g_delivery_admissions == 1 && g_dispatch_calls == 0 &&
         g_registered_finish == 1 && g_canceled_finish == 1);

  ResetReceiver();
  context = make_original_context();
  g_finish_full = true;
  const auto rejected_capacity =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(!rejected_capacity.invoked && g_dispatch_calls == 0 &&
         g_registered_finish == 1 && g_canceled_finish == 0 && !g_exception &&
         g_receiver->next_sequence == 1);

  ResetReceiver();
  context = make_original_context();
  g_receiver->next_sequence = std::numeric_limits<uint32_t>::max();
  const auto wrapped =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(wrapped.invoked && wrapped.framework_sequence == -1 &&
         g_receiver->next_sequence == 0);
  const auto after_wrap =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(after_wrap.invoked && after_wrap.framework_sequence == 0 &&
         g_receiver->next_sequence == 1);

  // Once dispatchInputEvent is invoked, Java owns recycling even when it
  // throws; native must report invocation and preserve the exception.
  ResetReceiver();
  context = make_original_context();
  g_throw_on_dispatch = true;
  const auto throwing_dispatch =
      darwin_art::input::DispatchFrameworkInputEventResultForReceiver(
          &env, context, Handle<jobject>(0x206));
  assert(throwing_dispatch.invoked && !throwing_dispatch.delivered &&
         g_dispatch_calls == 1 && g_exception);
  darwin_art::input::RetireReceiverResources(&env, g_receiver);
  assert(g_receiver->refs_cleaned && g_global_deletes == 2);
}
