#include "runtime/framework/input/packet_dispatch.h"

#include <jni.h>

#include <cassert>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>

namespace darwin_art::input {
// The packet owner only carries this lease opaquely. The production definition
// is supplied by receiver_jni_resources.cc; this test does not link that
// resource owner.
struct InputReceiver {};
}  // namespace darwin_art::input

namespace {
bool g_exception = false;
darwin_art::input::InputEventOrigin g_seen_origin;
bool g_constructor_exception = false;
bool g_exception_on_dispatch = false;
int g_find_classes = 0;
int g_dispatch_calls = 0;
int g_recycle_calls = 0;
int g_delete_calls = 0;
int g_jni_calls = 0;
int g_fail_call = 0;
float g_x = 0.0f;
float g_y = 0.0f;
jlong g_down_ms = 0;
jlong g_event_ms = 0;
jint g_motion_action = 0;
jint g_motion_source = 0;
jint g_key_action = 0;
jint g_key_code = 0;
jint g_key_repeat = 0;
jint g_key_meta = 0;
jint g_key_device = 0;
jint g_key_scan = 0;
jint g_key_flags = 0;
jint g_key_source = 0;
darwin_art::FrameworkInputEventDispatchResult g_dispatch_result;
std::shared_ptr<darwin_art::input::InputReceiver> g_expected_receiver;
darwin_art::input::InputRoutingRecipientHandle g_expected_recipient;

bool FailJniCall() {
  assert(!g_exception);  // No throwable JNI operation with a pending exception.
  if (++g_jni_calls == g_fail_call) {
    g_exception = true;
    return true;
  }
  return false;
}

jclass FindClass(JNIEnv*, const char* name) {
  if (FailJniCall()) return nullptr;
  ++g_find_classes;
  if (std::strcmp(name, "android/view/MotionEvent") == 0)
    return reinterpret_cast<jclass>(0x11);
  if (std::strcmp(name, "android/view/MotionEvent$PointerProperties") == 0)
    return reinterpret_cast<jclass>(0x12);
  if (std::strcmp(name, "android/view/MotionEvent$PointerCoords") == 0)
    return reinterpret_cast<jclass>(0x13);
  if (std::strcmp(name, "android/view/KeyEvent") == 0)
    return reinterpret_cast<jclass>(0x14);
  return nullptr;
}
jmethodID Method(JNIEnv*, jclass clazz, const char* name, const char*) {
  if (FailJniCall()) return nullptr;
  if (std::strcmp(name, "obtain") == 0) return reinterpret_cast<jmethodID>(0x21);
  if (std::strcmp(name, "recycle") == 0) return reinterpret_cast<jmethodID>(0x22);
  if (std::strcmp(name, "<init>") == 0)
    return reinterpret_cast<jmethodID>(clazz == reinterpret_cast<jclass>(0x14)
                                           ? 0x23 : 0x24);
  return nullptr;
}
jfieldID Field(JNIEnv*, jclass, const char* name, const char*) {
  if (FailJniCall()) return nullptr;
  if (std::strcmp(name, "x") == 0) return reinterpret_cast<jfieldID>(0x31);
  if (std::strcmp(name, "y") == 0) return reinterpret_cast<jfieldID>(0x32);
  return reinterpret_cast<jfieldID>(0x33);
}
jobject NewObjectV(JNIEnv*, jclass clazz, jmethodID, va_list args) {
  if (FailJniCall()) return nullptr;
  if (clazz == reinterpret_cast<jclass>(0x14)) {
    g_down_ms = va_arg(args, jlong);
    g_event_ms = va_arg(args, jlong);
    g_key_action = va_arg(args, jint);
    g_key_code = va_arg(args, jint);
    g_key_repeat = va_arg(args, jint);
    g_key_meta = va_arg(args, jint);
    g_key_device = va_arg(args, jint);
    g_key_scan = va_arg(args, jint);
    g_key_flags = va_arg(args, jint);
    g_key_source = va_arg(args, jint);
    if (g_constructor_exception) g_exception = true;
  }
  return reinterpret_cast<jobject>(0x41);
}
jobjectArray NewArray(JNIEnv*, jsize, jclass, jobject) {
  if (FailJniCall()) return nullptr;
  return reinterpret_cast<jobjectArray>(0x42);
}
void SetInt(JNIEnv*, jobject, jfieldID, jint) {}
void SetFloat(JNIEnv*, jobject, jfieldID field, jfloat value) {
  if (reinterpret_cast<uintptr_t>(field) == 0x31) g_x = value;
  if (reinterpret_cast<uintptr_t>(field) == 0x32) g_y = value;
}
void SetElement(JNIEnv*, jobjectArray, jsize, jobject) { (void)FailJniCall(); }
jobject CallStaticV(JNIEnv*, jclass, jmethodID, va_list args) {
  if (FailJniCall()) return nullptr;
  g_down_ms = va_arg(args, jlong);
  g_event_ms = va_arg(args, jlong);
  g_motion_action = va_arg(args, jint);
  (void)va_arg(args, jint);
  (void)va_arg(args, jobjectArray);
  (void)va_arg(args, jobjectArray);
  (void)va_arg(args, jint);
  (void)va_arg(args, jint);
  (void)va_arg(args, double);
  (void)va_arg(args, double);
  (void)va_arg(args, jint);
  (void)va_arg(args, jint);
  g_motion_source = va_arg(args, jint);
  if (g_constructor_exception) g_exception = true;
  return reinterpret_cast<jobject>(0x43);
}
void CallVoidV(JNIEnv*, jobject, jmethodID method, va_list) {
  if (method == reinterpret_cast<jmethodID>(0x22)) ++g_recycle_calls;
}
jboolean Exception(JNIEnv*) { return g_exception ? JNI_TRUE : JNI_FALSE; }
void Delete(JNIEnv*, jobject object) {
  if (object != nullptr) ++g_delete_calls;
}

darwin_art::FrameworkInputEventDispatchResult Dispatch(
    JNIEnv*, jobject, jobject) {
  ++g_dispatch_calls;
  if (g_exception_on_dispatch) g_exception = true;
  return g_dispatch_result;
}

darwin_art::FrameworkInputEventDispatchResult DispatchOriginal(
    JNIEnv* env,
    const darwin_art::input::OriginalInputReceiverDispatchContext& context,
    jobject event) {
  assert(context.receiver == g_expected_receiver);
  assert(context.recipient == g_expected_recipient);
  g_seen_origin = context.origin;
  return Dispatch(env, nullptr, event);
}

void Reset() {
  g_exception = false;
  g_constructor_exception = false;
  g_exception_on_dispatch = false;
  g_find_classes = 0;
  g_dispatch_calls = 0;
  g_recycle_calls = 0;
  g_delete_calls = 0;
  g_jni_calls = 0;
  g_fail_call = 0;
  g_x = 0.0f;
  g_y = 0.0f;
  g_down_ms = 0;
  g_event_ms = 0;
  g_motion_action = 0;
  g_motion_source = 0;
  g_key_action = 0;
  g_key_code = 0;
  g_key_repeat = 0;
  g_key_meta = 0;
  g_key_device = 0;
  g_key_scan = 0;
  g_key_flags = 0;
  g_key_source = 0;
  g_dispatch_result = {};
}

darwin_art::DarwinArtInputPacket PointerPacket() {
  darwin_art::DarwinArtInputPacket packet;
  packet.kind = darwin_art::DarwinArtInputPacketKind::kPointer;
  packet.pointer.version = 2;
  packet.pointer.size = sizeof(DarwinArtPointerEventV2);
  packet.pointer.action = DARWIN_ART_POINTER_MOVE;
  packet.pointer.sequence = (1ULL << 48) + 9;
  packet.pointer.down_time_nanos = 4000000;
  packet.pointer.event_time_nanos = 9000000;
  packet.pointer.x = 12.5f;
  packet.pointer.y = 18.25f;
  packet.pointer.pointer_count = 1;
  return packet;
}
}

int main() {
  JNINativeInterface table{};
  table.FindClass = FindClass;
  table.GetMethodID = Method;
  table.GetFieldID = Field;
  table.GetStaticMethodID = Method;
  table.NewObjectV = NewObjectV;
  table.NewObjectArray = NewArray;
  table.SetIntField = SetInt;
  table.SetFloatField = SetFloat;
  table.SetObjectArrayElement = SetElement;
  table.CallStaticObjectMethodV = CallStaticV;
  table.CallVoidMethodV = CallVoidV;
  table.ExceptionCheck = Exception;
  table.DeleteLocalRef = Delete;
  JNIEnv env{&table};
  Reset();
  g_expected_receiver = std::make_shared<darwin_art::input::InputReceiver>();
  auto routing = darwin_art::input::InputRoutingHandle(
      reinterpret_cast<darwin_art::input::InputRoutingState*>(0x701),
      [](darwin_art::input::InputRoutingState*) {});
  g_expected_recipient = std::make_shared<const darwin_art::input::InputRoutingRecipient>(
      routing, 17, darwin_art::input::InputRoutingEndpointHandle{});
  darwin_art::input::OriginalInputReceiverDispatchContext original_context{
      g_expected_receiver, g_expected_recipient, DispatchOriginal,
      {darwin_art::input::ReceiverPacketOrigin::kImportedChannel, 0}};
  g_dispatch_result = {.invoked = true, .delivered = true, .acknowledged = true,
                       .handled = true, .framework_sequence = 17};
  const auto accepted = darwin_art::input::DispatchInputPacketToReceiver(
      &env, original_context, PointerPacket());
  assert(accepted.invoked && accepted.delivered && accepted.acknowledged &&
         accepted.handled && accepted.framework_sequence == 17);
  assert(g_dispatch_calls == 1 && g_recycle_calls == 0);
  assert(g_seen_origin.kind == darwin_art::input::ReceiverPacketOrigin::kImportedChannel &&
         g_seen_origin.sequence == (1ULL << 48) + 9);
  assert(g_x == 12.5f && g_y == 18.25f);
  assert(g_down_ms == 4 && g_event_ms == 9 &&
         g_motion_action == DARWIN_ART_POINTER_MOVE && g_motion_source == 0x1002);

  Reset();
  g_dispatch_result = {.invoked = true, .delivered = false};
  g_exception_on_dispatch = true;
  const auto original_thrown =
      darwin_art::input::DispatchInputPacketToReceiver(
          &env, original_context, PointerPacket());
  assert(original_thrown.invoked && !original_thrown.delivered &&
         g_dispatch_calls == 1 && g_recycle_calls == 0 && g_exception);

  Reset();
  darwin_art::DarwinArtInputPacket key_packet;
  key_packet.kind = darwin_art::DarwinArtInputPacketKind::kKey;
  key_packet.key.version = 1;
  key_packet.key.size = sizeof(DarwinArtKeyEventV1);
  key_packet.key.action = 1;
  key_packet.key.sequence = (1ULL << 44) + 12;
  key_packet.key.down_time_nanos = 11000000;
  key_packet.key.event_time_nanos = 17000000;
  key_packet.key.key_code = 29;
  key_packet.key.repeat_count = 2;
  key_packet.key.meta_state = 8;
  key_packet.key.device_id = 3;
  key_packet.key.scan_code = 44;
  key_packet.key.flags = 0x40;
  key_packet.key.source = 0x101;
  g_dispatch_result = {.invoked = true, .delivered = true, .acknowledged = true};
  const auto key_result = darwin_art::input::DispatchInputPacketToReceiver(
      &env, original_context, key_packet);
  assert(key_result.invoked && key_result.delivered && g_dispatch_calls == 1);
  assert(g_seen_origin.sequence == key_packet.key.sequence);
  assert(g_down_ms == 11 && g_event_ms == 17 && g_key_action == 1);
  assert(g_key_code == 29 && g_key_repeat == 2 && g_key_meta == 8);
  assert(g_key_device == 3 && g_key_scan == 44 && g_key_flags == 0x40);
  assert(g_key_source == 0x101 && g_recycle_calls == 0);

  Reset();
  g_dispatch_result = {.invoked = true, .delivered = true, .acknowledged = false};
  const auto deferred = darwin_art::input::DispatchInputPacketToReceiver(
      &env, original_context, PointerPacket());
  assert(deferred.invoked && deferred.delivered && !deferred.acknowledged &&
         g_dispatch_calls == 1);
  assert(g_recycle_calls == 0);  // AOSP owns recycling after invocation.

  Reset();
  g_dispatch_result = {.invoked = true, .delivered = false, .acknowledged = true};
  g_exception_on_dispatch = true;
  const auto thrown = darwin_art::input::DispatchInputPacketToReceiver(
      &env, original_context, PointerPacket());
  assert(thrown.invoked && !thrown.delivered && g_dispatch_calls == 1);
  assert(g_recycle_calls == 0 && g_exception);  // Handoff forbids native recycling.

  Reset();
  g_constructor_exception = true;
  const auto constructor_failed = darwin_art::input::DispatchInputPacketToReceiver(
      &env, original_context, PointerPacket());
  assert(!constructor_failed.invoked && g_dispatch_calls == 0);
  assert(g_recycle_calls == 0 && g_delete_calls >= 4 && g_exception);

  Reset();
  g_dispatch_result = {};
  g_exception = true;
  const auto failed = darwin_art::input::DispatchInputPacketToReceiver(
      &env, original_context, PointerPacket());
  assert(!failed.invoked && g_dispatch_calls == 0 && g_recycle_calls == 0);
  assert(g_exception && g_jni_calls == 0 && g_delete_calls == 0);

  // Fail each throwable construction step in turn. Cleanup may delete local
  // references, but must not perform another lookup/allocation with the
  // original exception pending or invoke framework dispatch.
  Reset();
  g_dispatch_result = {.invoked = true, .delivered = true};
  (void)darwin_art::input::DispatchInputPacketToReceiver(
      &env, original_context, PointerPacket());
  const int construction_steps = g_jni_calls;
  for (int step = 1; step <= construction_steps; ++step) {
    Reset();
    g_fail_call = step;
    const auto failure = darwin_art::input::DispatchInputPacketToReceiver(
        &env, original_context, PointerPacket());
    assert(!failure.invoked && g_dispatch_calls == 0 && g_exception);
    assert(g_jni_calls == step && g_recycle_calls == 0);
  }

  std::puts("packet dispatch: actual event construction, ACK/deferred ownership and exception preservation PASS");
}
