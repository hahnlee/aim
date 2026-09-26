#include "key_fallback.h"

namespace darwin_art::input {
namespace {

constexpr uint32_t kActionDown = 0;     // KeyEvent.ACTION_DOWN
constexpr uint32_t kActionUp = 1;       // KeyEvent.ACTION_UP
constexpr uint32_t kKeycodeBack = 4;    // KeyEvent.KEYCODE_BACK
constexpr uint32_t kKeycodeEscape = 111;
// KeyEvent.META_MODIFIER_MASK: shift, alt, ctrl, meta, sym and function.
constexpr uint32_t kMetaModifierMask = 0x000770ff;
// injectBackGesture: VIRTUAL_KEYBOARD, FLAG_FROM_SYSTEM | FLAG_VIRTUAL_HARD_KEY.
constexpr int32_t kVirtualKeyboard = -1;
constexpr uint32_t kInjectedBackFlags = 0x48;

// Clears and reports a pending exception from this owner's own lookup calls;
// a device without a loadable map simply has no fallback, as in AOSP.
bool ClearLookupFailure(JNIEnv* env) {
  if (!env->ExceptionCheck()) return false;
  env->ExceptionDescribe();
  env->ExceptionClear();
  return true;
}

}  // namespace

void KeyFallbackOwner::Dispatching(uint32_t sequence, const DarwinArtKeyEventV1& key) {
  std::lock_guard<std::mutex> lock(mutex_);
  // Finishes normally arrive in order; a receiver that never finishes must
  // not grow this without bound.
  if (dispatched_.size() >= kMaxInFlight) dispatched_.erase(dispatched_.begin());
  dispatched_[sequence] = key;
}

void KeyFallbackOwner::Abandoned(uint32_t sequence) {
  std::lock_guard<std::mutex> lock(mutex_);
  dispatched_.erase(sequence);
}

std::vector<DarwinArtKeyEventV1> KeyFallbackOwner::BackGesture(
    const DarwinArtKeyEventV1& key) {
  if (key.key_code != kKeycodeEscape || key.action != kActionDown ||
      key.repeat_count != 0 || (key.meta_state & kMetaModifierMask) != 0)
    return {};
  DarwinArtKeyEventV1 back = key;
  back.key_code = kKeycodeBack;
  back.scan_code = 0;
  back.meta_state = 0;
  back.repeat_count = 0;
  back.device_id = kVirtualKeyboard;
  back.flags = kInjectedBackFlags;
  back.unicode_char = 0;
  back.down_time_nanos = key.event_time_nanos;
  DarwinArtKeyEventV1 up = back;
  up.action = kActionUp;
  return {back, up};
}

std::vector<DarwinArtKeyEventV1> KeyFallbackOwner::Finished(JNIEnv* env,
                                                            uint32_t sequence,
                                                            bool handled) {
  DarwinArtKeyEventV1 key{};
  Active active{};
  bool has_active = false;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    const auto found = dispatched_.find(sequence);
    if (found == dispatched_.end()) return {};
    key = found->second;
    dispatched_.erase(found);
    if ((key.flags & kFlagFallback) != 0) return {};
    const auto current = active_.find(key.key_code);
    if (current != active_.end()) {
      active = current->second;
      has_active = true;
      if (key.action == kActionUp) active_.erase(current);
    }
  }
  if (key.action == kActionUp) {
    if (!has_active) return {};
    DarwinArtKeyEventV1 up = key;
    up.key_code = active.key_code;
    up.meta_state = active.meta_state;
    up.flags |= kFlagFallback | (handled ? kFlagCanceled : 0);
    return {up};
  }
  if (key.action != kActionDown || handled) return {};
  // The policy's key gestures come before KeyCharacterMap fallbacks.
  auto gesture = BackGesture(key);
  if (!gesture.empty()) return gesture;
  if (!has_active) {
    if (env == nullptr ||
        !LookupKeyFallback(env, key.device_id, key.key_code, key.meta_state,
                           &active.key_code, &active.meta_state))
      return {};
    std::lock_guard<std::mutex> lock(mutex_);
    active_[key.key_code] = active;
  }
  DarwinArtKeyEventV1 down = key;
  down.key_code = active.key_code;
  down.meta_state = active.meta_state;
  down.flags |= kFlagFallback;
  return {down};
}

bool LookupKeyFallback(JNIEnv* env, int32_t device_id, uint32_t key_code,
                       uint32_t meta_state, uint32_t* fallback_key,
                       uint32_t* fallback_meta) {
  if (env == nullptr || env->ExceptionCheck()) return false;
  jclass map_class = env->FindClass("android/view/KeyCharacterMap");
  if (ClearLookupFailure(env) || map_class == nullptr) return false;
  jmethodID load = env->GetStaticMethodID(map_class, "load",
                                          "(I)Landroid/view/KeyCharacterMap;");
  jmethodID get_fallback =
      load == nullptr ? nullptr
                      : env->GetMethodID(map_class, "getFallbackAction",
                                         "(II)Landroid/view/KeyCharacterMap$FallbackAction;");
  if (ClearLookupFailure(env) || get_fallback == nullptr) {
    env->DeleteLocalRef(map_class);
    return false;
  }
  jobject map = env->CallStaticObjectMethod(map_class, load, device_id);
  env->DeleteLocalRef(map_class);
  if (ClearLookupFailure(env) || map == nullptr) return false;
  jobject action = env->CallObjectMethod(map, get_fallback, static_cast<jint>(key_code),
                                         static_cast<jint>(meta_state));
  env->DeleteLocalRef(map);
  if (ClearLookupFailure(env) || action == nullptr) return false;
  jclass action_class = env->GetObjectClass(action);
  jfieldID key_field = env->GetFieldID(action_class, "keyCode", "I");
  jfieldID meta_field = key_field == nullptr ? nullptr
                                             : env->GetFieldID(action_class, "metaState", "I");
  jmethodID recycle = meta_field == nullptr ? nullptr
                                            : env->GetMethodID(action_class, "recycle", "()V");
  env->DeleteLocalRef(action_class);
  bool found = false;
  if (!ClearLookupFailure(env) && recycle != nullptr) {
    *fallback_key = static_cast<uint32_t>(env->GetIntField(action, key_field));
    *fallback_meta = static_cast<uint32_t>(env->GetIntField(action, meta_field));
    env->CallVoidMethod(action, recycle);
    found = !ClearLookupFailure(env) && *fallback_key != 0;
  }
  env->DeleteLocalRef(action);
  return found;
}

}  // namespace darwin_art::input
