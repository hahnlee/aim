#include "darwin_libcore_natives.h"

#include <unicode/uchar.h>
#include <unicode/uloc.h>
#include <unicode/ulocdata.h>
#include <unicode/uversion.h>

#include <bit>
#include <cmath>
#include <cstdint>
#include <iterator>

namespace aim {
namespace {

jint FloatToRawIntBits(JNIEnv*, jclass, jfloat value) {
  return std::bit_cast<std::int32_t>(value);
}

jfloat IntBitsToFloat(JNIEnv*, jclass, jint bits) {
  return std::bit_cast<float>(static_cast<std::int32_t>(bits));
}

jlong DoubleToRawLongBits(JNIEnv*, jclass, jdouble value) {
  return std::bit_cast<std::int64_t>(value);
}

jdouble LongBitsToDouble(JNIEnv*, jclass, jlong bits) {
  return std::bit_cast<double>(static_cast<std::int64_t>(bits));
}

// Keep the conventional named-JNI entrypoints available as a resolver
// fallback for core-oj images that arrive before the composed registration
// table is visible to ART.
extern "C" JNIEXPORT jint Java_java_lang_Float_floatToRawIntBits(JNIEnv* env,
                                                                  jclass cls,
                                                                  jfloat value) {
  return FloatToRawIntBits(env, cls, value);
}
extern "C" JNIEXPORT jfloat Java_java_lang_Float_intBitsToFloat(JNIEnv* env,
                                                                  jclass cls,
                                                                  jint bits) {
  return IntBitsToFloat(env, cls, bits);
}
extern "C" JNIEXPORT jlong Java_java_lang_Double_doubleToRawLongBits(JNIEnv* env,
                                                                       jclass cls,
                                                                       jdouble value) {
  return DoubleToRawLongBits(env, cls, value);
}
extern "C" JNIEXPORT jdouble Java_java_lang_Double_longBitsToDouble(JNIEnv* env,
                                                                       jclass cls,
                                                                       jlong bits) {
  return LongBitsToDouble(env, cls, bits);
}

jint CharacterDigit(JNIEnv*, jclass, jint code_point, jint radix) {
  return u_digit(code_point, radix);
}

jbyte CharacterDirectionality(JNIEnv*, jclass, jint code_point) {
  return static_cast<jbyte>(u_charDirection(code_point));
}

jstring CharacterName(JNIEnv* env, jclass, jint code_point) {
  const bool is_control =
      code_point <= 0x1f || (code_point >= 0x7f && code_point <= 0x9f);
  const UCharNameChoice choice =
      is_control ? U_EXTENDED_CHAR_NAME : U_UNICODE_CHAR_NAME;
  char name[BUFSIZ];
  UErrorCode status = U_ZERO_ERROR;
  const std::int32_t length =
      u_charName(code_point, choice, name, sizeof(name), &status);
  return U_FAILURE(status) || length == 0 ? nullptr : env->NewStringUTF(name);
}

jint CharacterNumericValue(JNIEnv*, jclass, jint code_point) {
  const double result = u_getNumericValue(code_point);
  if (result == U_NO_NUMERIC_VALUE) {
    return -1;
  }
  if (result < 0 || std::floor(result + 0.5) != result) {
    return -2;
  }
  return static_cast<jint>(result);
}

jint CharacterType(JNIEnv*, jclass, jint code_point) {
  return u_charType(code_point);
}

jboolean CharacterIsAlphabetic(JNIEnv*, jclass, jint code_point) {
  return u_hasBinaryProperty(code_point, UCHAR_ALPHABETIC);
}

jboolean CharacterIsDefined(JNIEnv*, jclass, jint code_point) {
  return u_isdefined(code_point);
}

jboolean CharacterIsDigit(JNIEnv*, jclass, jint code_point) {
  return u_isdigit(code_point);
}

jboolean CharacterIsIdentifierIgnorable(JNIEnv*, jclass, jint code_point) {
  return u_isIDIgnorable(code_point);
}

jboolean CharacterIsIdeographic(JNIEnv*, jclass, jint code_point) {
  return u_hasBinaryProperty(code_point, UCHAR_IDEOGRAPHIC);
}

jboolean CharacterIsLetter(JNIEnv*, jclass, jint code_point) {
  return u_isalpha(code_point);
}

jboolean CharacterIsLetterOrDigit(JNIEnv*, jclass, jint code_point) {
  return u_isalnum(code_point);
}

jboolean CharacterIsLowerCase(JNIEnv*, jclass, jint code_point) {
  return u_islower(code_point);
}

jboolean CharacterIsMirrored(JNIEnv*, jclass, jint code_point) {
  return u_isMirrored(code_point);
}

jboolean CharacterIsSpaceChar(JNIEnv*, jclass, jint code_point) {
  return u_isJavaSpaceChar(code_point);
}

jboolean CharacterIsTitleCase(JNIEnv*, jclass, jint code_point) {
  return u_istitle(code_point);
}

jboolean CharacterIsUnicodeIdentifierPart(JNIEnv*, jclass, jint code_point) {
  return u_isIDPart(code_point);
}

jboolean CharacterIsUnicodeIdentifierStart(JNIEnv*, jclass, jint code_point) {
  return u_isIDStart(code_point);
}

jboolean CharacterIsUpperCase(JNIEnv*, jclass, jint code_point) {
  return u_isupper(code_point);
}

jboolean CharacterIsWhitespace(JNIEnv*, jclass, jint code_point) {
  return u_isWhitespace(code_point);
}

jint CharacterToLowerCase(JNIEnv*, jclass, jint code_point) {
  return u_tolower(code_point);
}

jint CharacterToTitleCase(JNIEnv*, jclass, jint code_point) {
  return u_totitle(code_point);
}

jint CharacterToUpperCase(JNIEnv*, jclass, jint code_point) {
  return u_toupper(code_point);
}

jstring IcuVersionString(JNIEnv* env, const UVersionInfo version) {
  char text[U_MAX_VERSION_STRING_LENGTH];
  u_versionToString(version, text);
  return env->NewStringUTF(text);
}

jstring IcuGetIcuVersion(JNIEnv* env, jclass) {
  UVersionInfo version;
  u_getVersion(version);
  return IcuVersionString(env, version);
}

jstring IcuGetUnicodeVersion(JNIEnv* env, jclass) {
  UVersionInfo version;
  u_getUnicodeVersion(version);
  return IcuVersionString(env, version);
}

jstring IcuGetCldrVersion(JNIEnv* env, jclass) {
  UVersionInfo version;
  UErrorCode status = U_ZERO_ERROR;
  ulocdata_getCLDRVersion(version, &status);
  return U_SUCCESS(status) ? IcuVersionString(env, version) : nullptr;
}

jobjectArray IcuStringArray(JNIEnv* env, const char* const* values) {
  jclass string_class = env->FindClass("java/lang/String");
  if (string_class == nullptr) return nullptr;
  jsize count = 0;
  while (values != nullptr && values[count] != nullptr) ++count;
  jobjectArray result = env->NewObjectArray(count, string_class, nullptr);
  for (jsize i = 0; result != nullptr && i < count; ++i) {
    jstring value = env->NewStringUTF(values[i]);
    if (value == nullptr) break;
    env->SetObjectArrayElement(result, i, value);
    env->DeleteLocalRef(value);
  }
  env->DeleteLocalRef(string_class);
  return result;
}

jobjectArray IcuGetIsoCountries(JNIEnv* env, jclass) {
  return IcuStringArray(env, uloc_getISOCountries());
}

jobjectArray IcuGetIsoLanguages(JNIEnv* env, jclass) {
  return IcuStringArray(env, uloc_getISOLanguages());
}

bool Register(JNIEnv* env, const char* class_name,
              const JNINativeMethod* methods, jint method_count) {
  jclass klass = env->FindClass(class_name);
  if (klass == nullptr) {
    return false;
  }
  const bool registered =
      env->RegisterNatives(klass, methods, method_count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return registered;
}

}  // namespace

bool RegisterLibcoreCharacterNatives(JNIEnv* env) {
  JNINativeMethod character_methods[] = {
      {const_cast<char*>("digitImpl"), const_cast<char*>("(II)I"),
       reinterpret_cast<void*>(&CharacterDigit)},
      {const_cast<char*>("getDirectionalityImpl"), const_cast<char*>("(I)B"),
       reinterpret_cast<void*>(&CharacterDirectionality)},
      {const_cast<char*>("getNameImpl"),
       const_cast<char*>("(I)Ljava/lang/String;"),
       reinterpret_cast<void*>(&CharacterName)},
      {const_cast<char*>("getNumericValueImpl"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&CharacterNumericValue)},
      {const_cast<char*>("getTypeImpl"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&CharacterType)},
      {const_cast<char*>("isAlphabeticImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsAlphabetic)},
      {const_cast<char*>("isDefinedImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsDefined)},
      {const_cast<char*>("isDigitImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsDigit)},
      {const_cast<char*>("isIdentifierIgnorableImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsIdentifierIgnorable)},
      {const_cast<char*>("isIdeographicImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsIdeographic)},
      {const_cast<char*>("isLetterImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsLetter)},
      {const_cast<char*>("isLetterOrDigitImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsLetterOrDigit)},
      {const_cast<char*>("isLowerCaseImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsLowerCase)},
      {const_cast<char*>("isMirroredImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsMirrored)},
      {const_cast<char*>("isSpaceCharImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsSpaceChar)},
      {const_cast<char*>("isTitleCaseImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsTitleCase)},
      {const_cast<char*>("isUnicodeIdentifierPartImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsUnicodeIdentifierPart)},
      {const_cast<char*>("isUnicodeIdentifierStartImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsUnicodeIdentifierStart)},
      {const_cast<char*>("isUpperCaseImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsUpperCase)},
      {const_cast<char*>("isWhitespaceImpl"), const_cast<char*>("(I)Z"),
       reinterpret_cast<void*>(&CharacterIsWhitespace)},
      {const_cast<char*>("toLowerCaseImpl"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&CharacterToLowerCase)},
      {const_cast<char*>("toTitleCaseImpl"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&CharacterToTitleCase)},
      {const_cast<char*>("toUpperCaseImpl"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(&CharacterToUpperCase)},
  };
  JNINativeMethod float_methods[] = {
      {const_cast<char*>("floatToRawIntBits"), const_cast<char*>("(F)I"),
       reinterpret_cast<void*>(&FloatToRawIntBits)},
      {const_cast<char*>("intBitsToFloat"), const_cast<char*>("(I)F"),
       reinterpret_cast<void*>(&IntBitsToFloat)},
  };
  JNINativeMethod double_methods[] = {
      {const_cast<char*>("doubleToRawLongBits"), const_cast<char*>("(D)J"),
       reinterpret_cast<void*>(&DoubleToRawLongBits)},
      {const_cast<char*>("longBitsToDouble"), const_cast<char*>("(J)D"),
       reinterpret_cast<void*>(&LongBitsToDouble)},
  };
  return Register(env, "java/lang/Character", character_methods,
                  static_cast<jint>(std::size(character_methods))) &&
         Register(env, "java/lang/Float", float_methods,
                  static_cast<jint>(std::size(float_methods))) &&
         Register(env, "java/lang/Double", double_methods,
                  static_cast<jint>(std::size(double_methods)));
}

bool RegisterLibcoreIcuNatives(JNIEnv* env) {
  JNINativeMethod icu_methods[] = {
      {const_cast<char*>("getIcuVersion"), const_cast<char*>("()Ljava/lang/String;"),
       reinterpret_cast<void*>(&IcuGetIcuVersion)},
      {const_cast<char*>("getUnicodeVersion"), const_cast<char*>("()Ljava/lang/String;"),
       reinterpret_cast<void*>(&IcuGetUnicodeVersion)},
      {const_cast<char*>("getCldrVersion"), const_cast<char*>("()Ljava/lang/String;"),
       reinterpret_cast<void*>(&IcuGetCldrVersion)},
      {const_cast<char*>("getISOCountriesNative"),
       const_cast<char*>("()[Ljava/lang/String;"),
       reinterpret_cast<void*>(&IcuGetIsoCountries)},
      {const_cast<char*>("getISOLanguagesNative"),
       const_cast<char*>("()[Ljava/lang/String;"),
       reinterpret_cast<void*>(&IcuGetIsoLanguages)},
  };
  return Register(env, "libcore/icu/ICU", icu_methods,
                  static_cast<jint>(std::size(icu_methods)));
}

}  // namespace aim
