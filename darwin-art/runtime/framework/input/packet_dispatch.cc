#include "packet_dispatch.h"

#include <cstdio>
#include <cstdlib>
#include <unistd.h>

namespace darwin_art::input {
namespace {

jobject CreateMotionEvent(JNIEnv* env, const DarwinArtPointerEventV2& packet) {
  if (env == nullptr) return nullptr;
  jclass motion = env->FindClass("android/view/MotionEvent");
  if (env->ExceptionCheck()) {
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jclass properties = env->FindClass("android/view/MotionEvent$PointerProperties");
  if (env->ExceptionCheck()) {
    if (properties != nullptr) env->DeleteLocalRef(properties);
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jclass coords = env->FindClass("android/view/MotionEvent$PointerCoords");
  if (env->ExceptionCheck()) {
    if (coords != nullptr) env->DeleteLocalRef(coords);
    if (properties != nullptr) env->DeleteLocalRef(properties);
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jmethodID properties_init = properties == nullptr
                                  ? nullptr : env->GetMethodID(properties, "<init>", "()V");
  jmethodID coords_init = coords == nullptr || env->ExceptionCheck()
                              ? nullptr : env->GetMethodID(coords, "<init>", "()V");
  if (env->ExceptionCheck()) {
    if (coords != nullptr) env->DeleteLocalRef(coords);
    if (properties != nullptr) env->DeleteLocalRef(properties);
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jfieldID id_field = properties == nullptr ? nullptr : env->GetFieldID(properties, "id", "I");
  jfieldID tool_field = properties == nullptr || env->ExceptionCheck()
                            ? nullptr : env->GetFieldID(properties, "toolType", "I");
  if (env->ExceptionCheck()) {
    if (coords != nullptr) env->DeleteLocalRef(coords);
    if (properties != nullptr) env->DeleteLocalRef(properties);
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jfieldID x_field = coords == nullptr ? nullptr : env->GetFieldID(coords, "x", "F");
  jfieldID y_field = coords == nullptr || env->ExceptionCheck()
                            ? nullptr : env->GetFieldID(coords, "y", "F");
  jfieldID pressure_field = coords == nullptr || env->ExceptionCheck()
                                ? nullptr : env->GetFieldID(coords, "pressure", "F");
  jfieldID size_field = coords == nullptr || env->ExceptionCheck()
                               ? nullptr : env->GetFieldID(coords, "size", "F");
  if (env->ExceptionCheck()) {
    if (coords != nullptr) env->DeleteLocalRef(coords);
    if (properties != nullptr) env->DeleteLocalRef(properties);
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jmethodID obtain = motion == nullptr ? nullptr : env->GetStaticMethodID(
      motion, "obtain",
      "(JJII[Landroid/view/MotionEvent$PointerProperties;"
      "[Landroid/view/MotionEvent$PointerCoords;IIFFIIIII)"
      "Landroid/view/MotionEvent;");
  if (obtain == nullptr || properties_init == nullptr || coords_init == nullptr ||
      id_field == nullptr || tool_field == nullptr || x_field == nullptr ||
      y_field == nullptr || pressure_field == nullptr || size_field == nullptr ||
      env->ExceptionCheck()) {
    if (coords != nullptr) env->DeleteLocalRef(coords);
    if (properties != nullptr) env->DeleteLocalRef(properties);
    if (motion != nullptr) env->DeleteLocalRef(motion);
    return nullptr;
  }
  jobject pointer = env->NewObject(properties, properties_init);
  jobject point = env->ExceptionCheck() ? nullptr : env->NewObject(coords, coords_init);
  jobjectArray pointer_array = env->ExceptionCheck()
                                  ? nullptr : env->NewObjectArray(1, properties, nullptr);
  jobjectArray coords_array = env->ExceptionCheck()
                                 ? nullptr : env->NewObjectArray(1, coords, nullptr);
  if (pointer == nullptr || point == nullptr || pointer_array == nullptr ||
      coords_array == nullptr || env->ExceptionCheck()) {
    if (coords_array != nullptr) env->DeleteLocalRef(coords_array);
    if (pointer_array != nullptr) env->DeleteLocalRef(pointer_array);
    if (point != nullptr) env->DeleteLocalRef(point);
    if (pointer != nullptr) env->DeleteLocalRef(pointer);
    env->DeleteLocalRef(coords);
    env->DeleteLocalRef(properties);
    env->DeleteLocalRef(motion);
    return nullptr;
  }
  env->SetIntField(pointer, id_field, 0);
  env->SetIntField(pointer, tool_field, 1);
  env->SetFloatField(point, x_field, packet.x);
  env->SetFloatField(point, y_field, packet.y);
  env->SetFloatField(point, pressure_field,
                     packet.action == DARWIN_ART_POINTER_UP ||
                             packet.action == DARWIN_ART_POINTER_CANCEL ? 0.0f : 1.0f);
  env->SetFloatField(point, size_field, 1.0f);
  env->SetObjectArrayElement(pointer_array, 0, pointer);
  if (!env->ExceptionCheck()) env->SetObjectArrayElement(coords_array, 0, point);
  if (env->ExceptionCheck()) {
    env->DeleteLocalRef(coords_array);
    env->DeleteLocalRef(pointer_array);
    env->DeleteLocalRef(point);
    env->DeleteLocalRef(pointer);
    env->DeleteLocalRef(coords);
    env->DeleteLocalRef(properties);
    env->DeleteLocalRef(motion);
    return nullptr;
  }
  const uint64_t event_nanos = packet.event_time_nanos;
  const uint64_t down_nanos = packet.down_time_nanos == 0 ? event_nanos : packet.down_time_nanos;
  jobject event = env->CallStaticObjectMethod(
      motion, obtain, static_cast<jlong>(down_nanos / 1000000ULL),
      static_cast<jlong>(event_nanos / 1000000ULL), static_cast<jint>(packet.action), 1,
      pointer_array, coords_array, 0, 0, 1.0f, 1.0f, 0, 0, 0x1002, 0, 0);
  env->DeleteLocalRef(coords_array);
  env->DeleteLocalRef(pointer_array);
  env->DeleteLocalRef(point);
  env->DeleteLocalRef(pointer);
  env->DeleteLocalRef(coords);
  env->DeleteLocalRef(properties);
  env->DeleteLocalRef(motion);
  if (env->ExceptionCheck()) {
    // CallStaticObjectMethod may return a non-null local reference while also
    // reporting a constructor exception. The packet owner still owns that
    // local reference; do not leak it on the pre-handoff failure path.
    if (event != nullptr) env->DeleteLocalRef(event);
    return nullptr;
  }
  return event;
}

jobject CreateKeyEvent(JNIEnv* env, const DarwinArtKeyEventV1& packet) {
  if (env == nullptr) return nullptr;
  jclass key = env->FindClass("android/view/KeyEvent");
  if (env->ExceptionCheck()) {
    if (key != nullptr) env->DeleteLocalRef(key);
    return nullptr;
  }
  jmethodID init = key == nullptr || env->ExceptionCheck()
                           ? nullptr : env->GetMethodID(key, "<init>", "(JJIIIIIIII)V");
  if (init == nullptr || env->ExceptionCheck()) {
    if (key != nullptr) env->DeleteLocalRef(key);
    return nullptr;
  }
  const uint64_t event_nanos = packet.event_time_nanos;
  const uint64_t down_nanos = packet.down_time_nanos == 0 ? event_nanos : packet.down_time_nanos;
  jobject event = env->NewObject(
      key, init, static_cast<jlong>(down_nanos / 1000000ULL),
      static_cast<jlong>(event_nanos / 1000000ULL), static_cast<jint>(packet.action),
      static_cast<jint>(packet.key_code), static_cast<jint>(packet.repeat_count),
      static_cast<jint>(packet.meta_state), static_cast<jint>(packet.device_id),
      static_cast<jint>(packet.scan_code), static_cast<jint>(packet.flags),
      static_cast<jint>(packet.source));
  env->DeleteLocalRef(key);
  if (env->ExceptionCheck()) {
    if (event != nullptr) env->DeleteLocalRef(event);
    return nullptr;
  }
  return event;
}

void RecycleMotionEvent(JNIEnv* env, jobject event) {
  if (env == nullptr || event == nullptr || env->ExceptionCheck()) return;
  jclass motion = env->FindClass("android/view/MotionEvent");
  jmethodID recycle = motion == nullptr ? nullptr : env->GetMethodID(motion, "recycle", "()V");
  if (recycle != nullptr && !env->ExceptionCheck()) env->CallVoidMethod(event, recycle);
  if (motion != nullptr) env->DeleteLocalRef(motion);
}

template <typename DispatchFn>
FrameworkInputEventDispatchResult DispatchInputPacketImpl(
    JNIEnv* env, const darwin_art::DarwinArtInputPacket& packet,
    DispatchFn dispatch) {
  FrameworkInputEventDispatchResult result;
  if (env == nullptr || env->ExceptionCheck()) return result;
  jobject event = packet.kind == DarwinArtInputPacketKind::kPointer
                      ? CreateMotionEvent(env, packet.pointer)
                      : CreateKeyEvent(env, packet.key);
  if (event == nullptr) return result;
  result = dispatch(env, event);
  if (packet.kind == DarwinArtInputPacketKind::kPointer && !result.invoked)
    RecycleMotionEvent(env, event);
  env->DeleteLocalRef(event);
  if (std::getenv("DARWIN_ART_DEBUG_INPUT_LATENCY") != nullptr) {
    const auto& pointer = packet.pointer;
    const uint64_t sequence = packet.kind == DarwinArtInputPacketKind::kPointer
                                  ? pointer.sequence : packet.key.sequence;
    const uint32_t action = packet.kind == DarwinArtInputPacketKind::kPointer
                                ? pointer.action : packet.key.action;
    const float x = packet.kind == DarwinArtInputPacketKind::kPointer ? pointer.x : 0.0f;
    const float y = packet.kind == DarwinArtInputPacketKind::kPointer ? pointer.y : 0.0f;
    // MotionEvent construction preserves the existing touchscreen source;
    // report the source actually handed to Android rather than the host hint.
    const uint32_t source = packet.kind == DarwinArtInputPacketKind::kPointer
                                ? 0x1002u : packet.key.source;
    std::fprintf(stderr,
                 "ART Android Input dispatch-return pid=%d sequence=%llu action=%u x=%.3f y=%.3f source=0x%x "
                 "invoked=%d delivered=%d ack=%d framework_seq=%d handled=%s\n",
                 static_cast<int>(getpid()), static_cast<unsigned long long>(sequence), action, x,
                 y, source,
                 result.invoked ? 1 : 0, result.delivered ? 1 : 0,
                 result.acknowledged ? 1 : 0, result.framework_sequence,
                 result.acknowledged ? (result.handled ? "1" : "0") : "unknown");
  }
  return result;
}

}  // namespace

FrameworkInputEventDispatchResult DispatchInputPacketToReceiver(
    JNIEnv* env, const OriginalInputReceiverDispatchContext& context,
    const darwin_art::DarwinArtInputPacket& packet) {
  if (context.receiver == nullptr || context.dispatch == nullptr) return {};
  auto original = context;
  original.origin.sequence = packet.kind == DarwinArtInputPacketKind::kPointer
                                 ? packet.pointer.sequence : packet.key.sequence;
  return DispatchInputPacketImpl(
      env, packet,
      [&original](JNIEnv* callback_env, jobject event) {
        return original.dispatch(callback_env, original, event);
      });
}

}  // namespace darwin_art::input
