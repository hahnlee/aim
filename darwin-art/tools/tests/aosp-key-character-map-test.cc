#include <android/keycodes.h>
#include <binder/Parcel.h>
#include <input/KeyCharacterMap.h>

#include <cstdlib>
#include <iostream>

namespace {
void Require(bool condition, const char* contract) {
  if (!condition) {
    std::cerr << "aosp-key-character-map: FAIL " << contract << '\n';
    std::exit(1);
  }
}

void VerifyParcel(const android::KeyCharacterMap& map) {
  android::Parcel parcel;
  map.writeToParcel(&parcel);
  Require(parcel.errorCheck() == android::NO_ERROR, "write original map payload");
  Require(parcel.dataSize() > sizeof(int32_t), "not an identity-only payload");
  parcel.setDataPosition(0);
  auto restored = android::KeyCharacterMap::readFromParcel(&parcel);
  Require(restored != nullptr && *restored == map, "original Parcel round-trip");
  Require(parcel.dataPosition() == parcel.dataSize(), "consume complete map payload");
}
}  // namespace

// Test the original evaluator and serialization, not a fixture implementation
// of matching policy. JNI/device identity and physical APK input are separate
// acceptance gates; this component cannot establish either by itself.
int main(int argc, char** argv) {
  Require(argc == 3, "Generic.kcm and Virtual.kcm paths required");
  auto generic = android::KeyCharacterMap::load(
      argv[1], android::KeyCharacterMap::Format::BASE);
  Require(generic.ok(), "load original Generic.kcm");
  const auto& map = *generic.value();
  Require(map.getKeyboardType() == android::KeyCharacterMap::KeyboardType::FULL,
          "physical FULL keyboard");
  Require(map.getCharacter(AKEYCODE_A, 0) == u'a', "base letter");
  Require(map.getDisplayLabel(AKEYCODE_A) == u'A', "printed uppercase label");
  Require(map.getCharacter(AKEYCODE_A, AMETA_CTRL_ON) == 0,
          "Control shortcut must not insert a printable letter");
  Require(map.getCharacter(AKEYCODE_A, AMETA_META_ON) == 0,
          "Meta shortcut exact matching");
  Require(map.getCharacter(AKEYCODE_SEMICOLON, AMETA_SHIFT_ON) == u':',
          "Shift punctuation");
  Require(map.getCharacter(AKEYCODE_A, AMETA_CAPS_LOCK_ON) == u'A', "Caps Lock");
  Require(map.getCharacter(AKEYCODE_A, AMETA_SHIFT_ON | AMETA_CAPS_LOCK_ON) == u'a',
          "Shift plus Caps Lock");
  Require(map.getCharacter(AKEYCODE_C, AMETA_ALT_ON) == u'\u00e7',
          "explicit Alt character must not be blanket-suppressed");
  android::KeyCharacterMap::FallbackAction fallback;
  Require(map.getFallbackAction(AKEYCODE_SPACE, AMETA_CTRL_ON, &fallback) &&
              fallback.keyCode == AKEYCODE_LANGUAGE_SWITCH && fallback.metaState == 0,
          "original Control-Space fallback consumes Control modifier");
  Require(map.getFallbackAction(AKEYCODE_SPACE, AMETA_META_ON, &fallback) &&
              fallback.keyCode == AKEYCODE_SEARCH && fallback.metaState == 0,
          "original Meta-Space fallback consumes Meta modifier");
  VerifyParcel(map);
  auto virtual_map = android::KeyCharacterMap::load(
      argv[2], android::KeyCharacterMap::Format::BASE);
  Require(virtual_map.ok(), "load original Virtual.kcm");
  Require(virtual_map.value()->getKeyboardType() ==
              android::KeyCharacterMap::KeyboardType::FULL,
          "virtual fallback retains original FULL map");
  VerifyParcel(*virtual_map.value());
  std::cout << "aosp-key-character-map: PASS original evaluator and Parcel\n";
}
