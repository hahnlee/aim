#pragma once

#include <jni.h>

#include <cstdint>
#include <map>
#include <mutex>
#include <vector>

#include "../../../compat/darwin_framework_input_hint.h"

namespace darwin_art::input {

// The Android 16 unhandled-key policy (InputDispatcher's
// afterKeyEventLockedInterruptable calling PhoneWindowManager.
// dispatchUnhandledKey) for the keys this process dispatches to its own
// receivers:
//  - KeyGestureController.interceptUnhandledKey: an unhandled first ESCAPE
//    DOWN without modifiers is the BACK gesture, which injectBackGesture
//    delivers as a KEYCODE_BACK DOWN/UP pair from the virtual keyboard;
//  - otherwise the device KeyCharacterMap's fallback (Generic.kcm: ALT+ESCAPE
//    -> HOME) follows with FLAG_FALLBACK, and the original key's UP ends it,
//    cancelled when the app handled that UP.
class KeyFallbackOwner final {
 public:
  static constexpr uint32_t kFlagCanceled = 0x20;   // KeyEvent.FLAG_CANCELED
  static constexpr uint32_t kFlagFallback = 0x400;  // KeyEvent.FLAG_FALLBACK

  // A key is about to be dispatched with finish `sequence`.
  void Dispatching(uint32_t sequence, const DarwinArtKeyEventV1& key);
  // The dispatch was abandoned before the app took the event.
  void Abandoned(uint32_t sequence);
  // The app finished `sequence`: the keys to dispatch next, in order. Call
  // without input locks held; the fallback lookup runs Java code.
  std::vector<DarwinArtKeyEventV1> Finished(JNIEnv* env, uint32_t sequence,
                                            bool handled);

  // KeyGestureController.interceptUnhandledKey's BACK gesture for `key`, as
  // the key pair injectBackGesture injects; empty when `key` is not it.
  static std::vector<DarwinArtKeyEventV1> BackGesture(const DarwinArtKeyEventV1& key);

 private:
  struct Active {
    uint32_t key_code = 0;
    uint32_t meta_state = 0;
  };
  static constexpr size_t kMaxInFlight = 64;

  std::mutex mutex_;
  std::map<uint32_t, DarwinArtKeyEventV1> dispatched_;
  // Original key code -> the fallback dispatched for its DOWN.
  std::map<uint32_t, Active> active_;
};

// KeyCharacterMap.load(device).getFallbackAction(key, meta): false when the
// device's map has no fallback for the key.
bool LookupKeyFallback(JNIEnv* env, int32_t device_id, uint32_t key_code,
                       uint32_t meta_state, uint32_t* fallback_key,
                       uint32_t* fallback_meta);

}  // namespace darwin_art::input
