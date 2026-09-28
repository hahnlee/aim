#include "darwin_icu_natives.h"

#include <unicode/ucnv.h>
#include <unicode/ucnv_cb.h>

#include <algorithm>
#include <cstdint>
#include <iterator>
#include <string>
#include <string_view>
#include <vector>

namespace {

constexpr jint kReport = 0;
constexpr jint kIgnore = 1;
constexpr jint kReplace = 2;
constexpr std::size_t kMaxReplacementLength = 32;

UConverter* ToConverter(jlong handle) {
  return reinterpret_cast<UConverter*>(static_cast<std::uintptr_t>(handle));
}

const char* IcuCanonicalName(const char* name) {
  for (const char* standard : {"MIME", "IANA", ""}) {
    UErrorCode status = U_ZERO_ERROR;
    if (const char* canonical = ucnv_getCanonicalName(name, standard, &status);
        canonical != nullptr && U_SUCCESS(status)) {
      return canonical;
    }
  }
  UErrorCode status = U_ZERO_ERROR;
  const char* alias = ucnv_getAlias(name, 0, &status);
  return U_SUCCESS(status) ? alias : nullptr;
}

const char* JavaCanonicalName(const char* icu_name) {
  for (const char* standard : {"MIME", "IANA"}) {
    UErrorCode status = U_ZERO_ERROR;
    if (const char* name = ucnv_getStandardName(icu_name, standard, &status);
        name != nullptr && U_SUCCESS(status)) {
      return name;
    }
  }
  return icu_name;
}

jobject NativeConverterCharsetForName(JNIEnv* env, jclass,
                                      jstring charset_name) {
  if (charset_name == nullptr) {
    return nullptr;
  }
  const char* requested = env->GetStringUTFChars(charset_name, nullptr);
  if (requested == nullptr) {
    return nullptr;
  }
  const char* canonical = IcuCanonicalName(requested);
  env->ReleaseStringUTFChars(charset_name, requested);
  if (canonical == nullptr) {
    return nullptr;
  }

  UErrorCode status = U_ZERO_ERROR;
  UConverter* probe = ucnv_open(canonical, &status);
  if (U_FAILURE(status) || probe == nullptr) {
    return nullptr;
  }
  ucnv_close(probe);

  const char* java_name = JavaCanonicalName(canonical);
  UErrorCode alias_status = U_ZERO_ERROR;
  const std::uint16_t alias_count = ucnv_countAliases(canonical, &alias_status);
  jclass string_class = env->FindClass("java/lang/String");
  jobjectArray aliases =
      string_class == nullptr
          ? nullptr
          : env->NewObjectArray(U_SUCCESS(alias_status) ? alias_count : 0,
                                string_class, nullptr);
  if (aliases == nullptr) {
    env->DeleteLocalRef(string_class);
    return nullptr;
  }
  for (std::uint16_t index = 0; index < alias_count; ++index) {
    UErrorCode current_status = U_ZERO_ERROR;
    const char* alias = ucnv_getAlias(canonical, index, &current_status);
    if (alias == nullptr || U_FAILURE(current_status)) {
      continue;
    }
    jstring value = env->NewStringUTF(alias);
    env->SetObjectArrayElement(aliases, index, value);
    env->DeleteLocalRef(value);
  }

  jclass charset_icu = env->FindClass("com/android/icu/charset/CharsetICU");
  jmethodID constructor =
      charset_icu == nullptr
          ? nullptr
          : env->GetMethodID(
                charset_icu, "<init>",
                "(Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;)V");
  jstring java_canonical = env->NewStringUTF(java_name);
  const char* versioned =
      std::string_view(canonical) == "UTF-16" ? "UTF-16,version=2" : canonical;
  jstring icu_canonical = env->NewStringUTF(versioned);
  jobject result = constructor == nullptr
                       ? nullptr
                       : env->NewObject(charset_icu, constructor,
                                        java_canonical, icu_canonical, aliases);
  env->DeleteLocalRef(java_canonical);
  env->DeleteLocalRef(icu_canonical);
  env->DeleteLocalRef(charset_icu);
  env->DeleteLocalRef(aliases);
  env->DeleteLocalRef(string_class);
  return result;
}

jlong NativeConverterOpenConverter(JNIEnv* env, jclass, jstring charset_name) {
  if (charset_name == nullptr) {
    return 0;
  }
  const char* name = env->GetStringUTFChars(charset_name, nullptr);
  if (name == nullptr) {
    return 0;
  }
  UErrorCode status = U_ZERO_ERROR;
  UConverter* converter = ucnv_open(name, &status);
  env->ReleaseStringUTFChars(charset_name, name);
  return U_SUCCESS(status) ? reinterpret_cast<std::uintptr_t>(converter) : 0;
}

void NativeConverterCloseConverter(JNIEnv*, jclass, jlong handle) {
  ucnv_close(ToConverter(handle));
}

void NativeConverterResetCharToByte(JNIEnv*, jclass, jlong handle) {
  if (UConverter* converter = ToConverter(handle); converter != nullptr) {
    ucnv_resetFromUnicode(converter);
  }
}

void NativeConverterResetByteToChar(JNIEnv*, jclass, jlong handle) {
  if (UConverter* converter = ToConverter(handle); converter != nullptr) {
    ucnv_resetToUnicode(converter);
  }
}

jint NativeConverterGetMaxBytesPerChar(JNIEnv*, jclass, jlong handle) {
  UConverter* converter = ToConverter(handle);
  return converter == nullptr ? -1 : ucnv_getMaxCharSize(converter);
}

jfloat NativeConverterGetAveBytesPerChar(JNIEnv*, jclass, jlong handle) {
  UConverter* converter = ToConverter(handle);
  return converter == nullptr ? -1.0F
                              : (ucnv_getMaxCharSize(converter) +
                                 ucnv_getMinCharSize(converter)) /
                                    2.0F;
}

jfloat NativeConverterGetAveCharsPerByte(JNIEnv*, jclass, jlong handle) {
  UConverter* converter = ToConverter(handle);
  return converter == nullptr
             ? -1.0F
             : 1.0F / static_cast<jfloat>(ucnv_getMaxCharSize(converter));
}

jbyteArray NativeConverterGetSubstitutionBytes(JNIEnv* env, jclass,
                                               jlong handle) {
  UConverter* converter = ToConverter(handle);
  if (converter == nullptr) {
    return nullptr;
  }
  char bytes[kMaxReplacementLength];
  std::int8_t length = sizeof(bytes);
  UErrorCode status = U_ZERO_ERROR;
  ucnv_getSubstChars(converter, bytes, &length, &status);
  if (U_FAILURE(status)) {
    length = 0;
  }
  jbyteArray result = env->NewByteArray(length);
  if (result != nullptr && length != 0) {
    env->SetByteArrayRegion(result, 0, length,
                            reinterpret_cast<const jbyte*>(bytes));
  }
  return result;
}

UConverterFromUCallback EncoderCallback(jint mode) {
  switch (mode) {
    case kIgnore:
      return UCNV_FROM_U_CALLBACK_SKIP;
    case kReplace:
      return UCNV_FROM_U_CALLBACK_SUBSTITUTE;
    case kReport:
    default:
      return UCNV_FROM_U_CALLBACK_STOP;
  }
}

UConverterToUCallback DecoderCallback(jint mode) {
  switch (mode) {
    case kIgnore:
      return UCNV_TO_U_CALLBACK_SKIP;
    case kReplace:
      return UCNV_TO_U_CALLBACK_SUBSTITUTE;
    case kReport:
    default:
      return UCNV_TO_U_CALLBACK_STOP;
  }
}

void NativeConverterSetCallbackDecode(JNIEnv* env, jclass, jlong handle,
                                      jint on_malformed, jint on_unmappable,
                                      jstring replacement) {
  UConverter* converter = ToConverter(handle);
  if (converter == nullptr || replacement == nullptr) {
    return;
  }
  const jchar* chars = env->GetStringChars(replacement, nullptr);
  if (chars == nullptr) {
    return;
  }
  UErrorCode status = U_ZERO_ERROR;
  ucnv_setSubstString(converter,
                      reinterpret_cast<const UChar*>(chars),
                      env->GetStringLength(replacement),
                      &status);
  env->ReleaseStringChars(replacement, chars);
  if (U_FAILURE(status)) {
    return;
  }
  const jint mode = on_malformed == on_unmappable ? on_malformed : kReport;
  ucnv_setToUCallBack(converter, DecoderCallback(mode), nullptr, nullptr, nullptr,
                      &status);
}

void NativeConverterSetCallbackEncode(JNIEnv* env, jclass, jlong handle,
                                      jint on_malformed, jint on_unmappable,
                                      jbyteArray replacement) {
  UConverter* converter = ToConverter(handle);
  if (converter == nullptr || replacement == nullptr) {
    return;
  }
  const jsize length = env->GetArrayLength(replacement);
  std::vector<jbyte> bytes(static_cast<std::size_t>(length));
  env->GetByteArrayRegion(replacement, 0, length, bytes.data());
  UErrorCode status = U_ZERO_ERROR;
  ucnv_setSubstChars(converter, reinterpret_cast<const char*>(bytes.data()),
                     static_cast<std::int8_t>(length), &status);
  if (U_FAILURE(status)) {
    return;
  }
  // ICU exposes a single callback at this level. Android's full bridge uses a
  // dispatching callback to distinguish malformed from unmappable input. The
  // two actions are normally identical; prefer REPORT if callers differ.
  const jint mode = on_malformed == on_unmappable ? on_malformed : kReport;
  ucnv_setFromUCallBack(converter, EncoderCallback(mode), nullptr, nullptr,
                        nullptr, &status);
}

jint NativeConverterEncode(JNIEnv* env, jclass, jlong handle, jcharArray source,
                           jint source_end, jbyteArray target, jint target_end,
                           jintArray data, jboolean flush) {
  UConverter* converter = ToConverter(handle);
  if (converter == nullptr || source == nullptr || target == nullptr ||
      data == nullptr) {
    return U_ILLEGAL_ARGUMENT_ERROR;
  }
  jchar* source_elements = env->GetCharArrayElements(source, nullptr);
  jbyte* target_elements = env->GetByteArrayElements(target, nullptr);
  jint* offsets = env->GetIntArrayElements(data, nullptr);
  if (source_elements == nullptr || target_elements == nullptr ||
      offsets == nullptr) {
    if (source_elements != nullptr)
      env->ReleaseCharArrayElements(source, source_elements, JNI_ABORT);
    if (target_elements != nullptr)
      env->ReleaseByteArrayElements(target, target_elements, 0);
    if (offsets != nullptr) env->ReleaseIntArrayElements(data, offsets, 0);
    return U_MEMORY_ALLOCATION_ERROR;
  }

  const jint initial_source = offsets[0];
  const UChar* source_cursor =
      reinterpret_cast<const UChar*>(source_elements + initial_source);
  const UChar* source_limit =
      reinterpret_cast<const UChar*>(source_elements + source_end);
  char* target_cursor = reinterpret_cast<char*>(target_elements + offsets[1]);
  const char* target_limit =
      reinterpret_cast<const char*>(target_elements + target_end);
  UErrorCode status = U_ZERO_ERROR;
  ucnv_fromUnicode(converter, &target_cursor, target_limit, &source_cursor,
                   source_limit, nullptr, flush == JNI_TRUE, &status);
  offsets[0] = static_cast<jint>(
      source_cursor - reinterpret_cast<const UChar*>(source_elements) -
      initial_source);
  offsets[1] = static_cast<jint>(
      target_cursor - reinterpret_cast<const char*>(target_elements));

  env->ReleaseCharArrayElements(source, source_elements, JNI_ABORT);
  env->ReleaseByteArrayElements(target, target_elements, 0);
  env->ReleaseIntArrayElements(data, offsets, 0);
  return status;
}

jint NativeConverterDecode(JNIEnv* env, jclass, jlong handle, jbyteArray source,
                           jint source_end, jcharArray target, jint target_end,
                           jintArray data, jboolean flush) {
  UConverter* converter = ToConverter(handle);
  if (converter == nullptr || source == nullptr || target == nullptr ||
      data == nullptr) {
    return U_ILLEGAL_ARGUMENT_ERROR;
  }
  jbyte* source_elements = env->GetByteArrayElements(source, nullptr);
  jchar* target_elements = env->GetCharArrayElements(target, nullptr);
  jint* offsets = env->GetIntArrayElements(data, nullptr);
  if (source_elements == nullptr || target_elements == nullptr ||
      offsets == nullptr) {
    if (source_elements != nullptr)
      env->ReleaseByteArrayElements(source, source_elements, JNI_ABORT);
    if (target_elements != nullptr)
      env->ReleaseCharArrayElements(target, target_elements, 0);
    if (offsets != nullptr) env->ReleaseIntArrayElements(data, offsets, 0);
    return U_MEMORY_ALLOCATION_ERROR;
  }

  const jint initial_source = offsets[0];
  const jint initial_target = offsets[1];
  const char* source_cursor =
      reinterpret_cast<const char*>(source_elements + initial_source);
  const char* source_limit =
      reinterpret_cast<const char*>(source_elements + source_end);
  UChar* target_cursor =
      reinterpret_cast<UChar*>(target_elements + initial_target);
  const UChar* target_limit =
      reinterpret_cast<const UChar*>(target_elements + target_end);
  UErrorCode status = U_ZERO_ERROR;
  ucnv_toUnicode(converter, &target_cursor, target_limit, &source_cursor,
                 source_limit, nullptr, flush == JNI_TRUE, &status);
  offsets[0] = static_cast<jint>(
      source_cursor -
      reinterpret_cast<const char*>(source_elements + initial_source));
  offsets[1] = static_cast<jint>(
      target_cursor - reinterpret_cast<UChar*>(target_elements + initial_target));
  offsets[2] = 0;
  if (U_FAILURE(status)) {
    char invalid[kMaxReplacementLength];
    std::int8_t invalid_length = sizeof(invalid);
    UErrorCode invalid_status = U_ZERO_ERROR;
    ucnv_getInvalidChars(converter, invalid, &invalid_length, &invalid_status);
    if (U_SUCCESS(invalid_status)) {
      offsets[2] = invalid_length;
    }
  }

  env->ReleaseByteArrayElements(source, source_elements, JNI_ABORT);
  env->ReleaseCharArrayElements(target, target_elements, 0);
  env->ReleaseIntArrayElements(data, offsets, 0);
  return status;
}

void FreeNativeConverter(void* converter) {
  ucnv_close(static_cast<UConverter*>(converter));
}

jlong NativeConverterGetNativeFinalizer(JNIEnv*, jclass) {
  return reinterpret_cast<jlong>(&FreeNativeConverter);
}

bool Register(JNIEnv* env, const char* class_name, JNINativeMethod* methods,
              jint method_count) {
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

namespace aim {

bool RegisterIcuCharsetNatives(JNIEnv* env) {
  JNINativeMethod methods[] = {
      {const_cast<char*>("charsetForName"),
       const_cast<char*>("(Ljava/lang/String;)Ljava/nio/charset/Charset;"),
       reinterpret_cast<void*>(&NativeConverterCharsetForName)},
      {const_cast<char*>("openConverter"),
       const_cast<char*>("(Ljava/lang/String;)J"),
       reinterpret_cast<void*>(&NativeConverterOpenConverter)},
      {const_cast<char*>("closeConverter"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&NativeConverterCloseConverter)},
      {const_cast<char*>("resetCharToByte"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&NativeConverterResetCharToByte)},
      {const_cast<char*>("resetByteToChar"), const_cast<char*>("(J)V"),
       reinterpret_cast<void*>(&NativeConverterResetByteToChar)},
      {const_cast<char*>("getMaxBytesPerChar"), const_cast<char*>("(J)I"),
       reinterpret_cast<void*>(&NativeConverterGetMaxBytesPerChar)},
      {const_cast<char*>("getAveBytesPerChar"), const_cast<char*>("(J)F"),
       reinterpret_cast<void*>(&NativeConverterGetAveBytesPerChar)},
      {const_cast<char*>("getAveCharsPerByte"), const_cast<char*>("(J)F"),
       reinterpret_cast<void*>(&NativeConverterGetAveCharsPerByte)},
      {const_cast<char*>("getSubstitutionBytes"), const_cast<char*>("(J)[B"),
       reinterpret_cast<void*>(&NativeConverterGetSubstitutionBytes)},
      {const_cast<char*>("setCallbackEncode"), const_cast<char*>("(JII[B)V"),
       reinterpret_cast<void*>(&NativeConverterSetCallbackEncode)},
      {const_cast<char*>("setCallbackDecode"),
       const_cast<char*>("(JIILjava/lang/String;)V"),
       reinterpret_cast<void*>(&NativeConverterSetCallbackDecode)},
      {const_cast<char*>("encode"), const_cast<char*>("(J[CI[BI[IZ)I"),
       reinterpret_cast<void*>(&NativeConverterEncode)},
      {const_cast<char*>("decode"), const_cast<char*>("(J[BI[CI[IZ)I"),
       reinterpret_cast<void*>(&NativeConverterDecode)},
      {const_cast<char*>("getNativeFinalizer"), const_cast<char*>("()J"),
       reinterpret_cast<void*>(&NativeConverterGetNativeFinalizer)},
  };
  return Register(env, "com/android/icu/charset/NativeConverter", methods,
                  static_cast<jint>(std::size(methods)));
}

void ShutdownIcuCharsetNatives() {}

}  // namespace aim
