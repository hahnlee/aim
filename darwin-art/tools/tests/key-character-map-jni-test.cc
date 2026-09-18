#include "runtime/framework/input/key_character_map_jni.h"

#include <cassert>
#include <vector>

namespace {
bool pending = false;
bool event_throws = false;
bool map_throws = false;
int event_result = JNI_OK;
int map_result = JNI_OK;
bool factory_result = true;
std::vector<int> calls;
jboolean ExceptionCheck(JNIEnv*) { return pending ? JNI_TRUE : JNI_FALSE; }
void Reset() {
  pending = event_throws = map_throws = false;
  event_result = map_result = JNI_OK;
  factory_result = true;
  calls.clear();
}
}

// Test-only external registrar actors: exercise production ordering/admission,
// not a fake map evaluator or Parcel. Actual original evaluator/Parcel coverage
// is in aosp-key-character-map-runtime-test.sh and JNI compilation separately.
namespace android {
int register_android_view_KeyEvent(JNIEnv*) {
  calls.push_back(1);
  pending = event_throws;
  return event_result;
}
int register_android_view_KeyCharacterMap(JNIEnv*) {
  calls.push_back(2);
  pending = map_throws;
  return map_result;
}
}
namespace darwin_art::input {
bool RegisterSystemKeyboardMapsNatives(JNIEnv*) {
  calls.push_back(3);
  return factory_result;
}
}

int main() {
  JNINativeInterface_ table{};
  table.ExceptionCheck = ExceptionCheck;
  JNIEnv env{&table};
  using darwin_art::input::RegisterKeyCharacterMapNatives;
  assert(!RegisterKeyCharacterMapNatives(nullptr));
  pending = true;
  assert(!RegisterKeyCharacterMapNatives(&env) && calls.empty());
  Reset();
  assert(RegisterKeyCharacterMapNatives(&env));
  assert((calls == std::vector<int>{1, 2, 3}));
  Reset();
  event_result = JNI_ERR;
  assert(!RegisterKeyCharacterMapNatives(&env) && calls.size() == 1);
  Reset();
  event_throws = true;
  assert(!RegisterKeyCharacterMapNatives(&env) && pending && calls.size() == 1);
  Reset();
  map_result = JNI_ERR;
  assert(!RegisterKeyCharacterMapNatives(&env) && calls.size() == 2);
  Reset();
  map_throws = true;
  assert(!RegisterKeyCharacterMapNatives(&env) && pending && calls.size() == 2);
  Reset();
  factory_result = false;
  assert(!RegisterKeyCharacterMapNatives(&env) && calls.size() == 3);
}
