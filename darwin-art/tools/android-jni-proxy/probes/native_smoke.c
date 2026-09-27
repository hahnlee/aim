#include "darwin_art_jni_proxy.h"
#include "jni_slots.h"

#include <stdint.h>
#include <stdio.h>

typedef void (*RawSlot)(void);
typedef int32_t (*GetEnvFunction)(void*, void**, int32_t);
typedef int32_t (*GetVersionFunction)(void*);
typedef void* (*FindClassFunction)(void*, const char*);
typedef uint8_t (*IsAssignableFromFunction)(void*, void*, void*);
typedef int32_t (*RegisterNativesFunction)(void*, void*,
                                           const DarwinArtJniNativeMethod*,
                                           int32_t);
typedef int32_t (*ThrowNewFunction)(void*, void*, const char*);
typedef void* (*ExceptionOccurredFunction)(void*);
typedef void (*ExceptionClearFunction)(void*);
typedef int32_t (*PushLocalFrameFunction)(void*, int32_t);
typedef void* (*PopLocalFrameFunction)(void*, void*);
typedef uint8_t (*ExceptionCheckFunction)(void*);
typedef void* (*NewReferenceFunction)(void*, void*);
typedef void* (*GetFieldIdFunction)(void*, void*, const char*, const char*);
typedef void (*DeleteReferenceFunction)(void*, void*);
typedef void (*DeleteLocalRefFunction)(void*, void*);
typedef void* (*NewStringFunction)(void*, const uint16_t*, int32_t);
typedef int32_t (*GetStringLengthFunction)(void*, void*);
typedef const uint16_t* (*GetStringCharsFunction)(void*, void*, uint8_t*);
typedef void (*ReleaseStringCharsFunction)(void*, void*, const uint16_t*);
typedef void* (*NewStringUtfFunction)(void*, const char*);
typedef int32_t (*GetStringUtfLengthFunction)(void*, void*);
typedef const char* (*GetStringUtfCharsFunction)(void*, void*, uint8_t*);
typedef void (*ReleaseStringUtfCharsFunction)(void*, void*, const char*);
typedef int32_t (*GetArrayLengthFunction)(void*, void*);
typedef void* (*NewObjectArrayFunction)(void*, int32_t, void*, void*);
typedef void* (*GetObjectArrayElementFunction)(void*, void*, int32_t);
typedef void (*SetObjectArrayElementFunction)(void*, void*, int32_t, void*);
typedef void* (*NewByteArrayFunction)(void*, int32_t);
typedef int8_t* (*GetByteArrayElementsFunction)(void*, void*, uint8_t*);
typedef void (*ReleaseByteArrayElementsFunction)(void*, void*, int8_t*,
                                                  int32_t);
typedef void (*GetByteArrayRegionFunction)(void*, void*, int32_t, int32_t,
                                           int8_t*);
typedef void (*SetByteArrayRegionFunction)(void*, void*, int32_t, int32_t,
                                           const int8_t*);
typedef int32_t (*MonitorFunction)(void*, void*);
typedef int32_t (*GetJavaVmFunction)(void*, void**);

extern void darwin_art_jni_fixture_reset(void);
extern void* darwin_art_jni_fixture_vm(void);
extern int32_t darwin_art_jni_fixture_passed(void);

#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) return __LINE__;                                         \
  } while (0)

int main(void) {
  darwin_art_jni_fixture_reset();
  void* vm = darwin_art_jni_fixture_vm();
  CHECK(vm != NULL);
  const RawSlot* invoke = *(const RawSlot* const*)vm;
  CHECK(invoke[3] == NULL);
  GetEnvFunction get_env =
      (GetEnvFunction)invoke[DARWIN_ART_JNI_INVOKE_SLOT_GetEnv];
  void* env = (void*)(uintptr_t)1;
  CHECK(get_env(vm, &env, 0x00010008) == DARWIN_ART_JNI_EVERSION);
  CHECK(env == NULL);
  CHECK(get_env(vm, &env, DARWIN_ART_JNI_VERSION_1_2) == DARWIN_ART_JNI_OK);
  CHECK(env != NULL);
  env = NULL;
  CHECK(get_env(vm, &env, DARWIN_ART_JNI_VERSION_1_6) == DARWIN_ART_JNI_OK);
  CHECK(env != NULL);

  const RawSlot* native = *(const RawSlot* const*)env;
  // Only the four reserved slots are empty.
  for (uint32_t index = 0; index < DARWIN_ART_JNI_NATIVE_SLOT_COUNT; ++index) {
    CHECK((native[index] == NULL) == (index < 4));
  }
  GetVersionFunction get_version =
      (GetVersionFunction)native[DARWIN_ART_JNI_SLOT_GetVersion];
  FindClassFunction find_class =
      (FindClassFunction)native[DARWIN_ART_JNI_SLOT_FindClass];
  NewReferenceFunction get_superclass =
      (NewReferenceFunction)native[DARWIN_ART_JNI_SLOT_GetSuperclass];
  IsAssignableFromFunction is_assignable_from =
      (IsAssignableFromFunction)native[DARWIN_ART_JNI_SLOT_IsAssignableFrom];
  ThrowNewFunction throw_new =
      (ThrowNewFunction)native[DARWIN_ART_JNI_SLOT_ThrowNew];
  ExceptionOccurredFunction exception_occurred =
      (ExceptionOccurredFunction)native[DARWIN_ART_JNI_SLOT_ExceptionOccurred];
  ExceptionClearFunction exception_clear =
      (ExceptionClearFunction)native[DARWIN_ART_JNI_SLOT_ExceptionClear];
  PushLocalFrameFunction push_local_frame =
      (PushLocalFrameFunction)native[DARWIN_ART_JNI_SLOT_PushLocalFrame];
  PopLocalFrameFunction pop_local_frame =
      (PopLocalFrameFunction)native[DARWIN_ART_JNI_SLOT_PopLocalFrame];
  RegisterNativesFunction register_natives =
      (RegisterNativesFunction)native[DARWIN_ART_JNI_SLOT_RegisterNatives];
  MonitorFunction monitor_enter =
      (MonitorFunction)native[DARWIN_ART_JNI_SLOT_MonitorEnter];
  MonitorFunction monitor_exit =
      (MonitorFunction)native[DARWIN_ART_JNI_SLOT_MonitorExit];
  GetJavaVmFunction get_java_vm =
      (GetJavaVmFunction)native[DARWIN_ART_JNI_SLOT_GetJavaVM];
  ExceptionCheckFunction exception_check =
      (ExceptionCheckFunction)native[DARWIN_ART_JNI_SLOT_ExceptionCheck];
  DeleteLocalRefFunction delete_local_ref =
      (DeleteLocalRefFunction)native[DARWIN_ART_JNI_SLOT_DeleteLocalRef];
  NewReferenceFunction new_global_ref =
      (NewReferenceFunction)native[DARWIN_ART_JNI_SLOT_NewGlobalRef];
  DeleteReferenceFunction delete_global_ref =
      (DeleteReferenceFunction)native[DARWIN_ART_JNI_SLOT_DeleteGlobalRef];
  NewReferenceFunction new_local_ref =
      (NewReferenceFunction)native[DARWIN_ART_JNI_SLOT_NewLocalRef];
  NewReferenceFunction get_object_class =
      (NewReferenceFunction)native[DARWIN_ART_JNI_SLOT_GetObjectClass];
  GetFieldIdFunction get_field_id =
      (GetFieldIdFunction)native[DARWIN_ART_JNI_SLOT_GetFieldID];
  GetFieldIdFunction get_method_id =
      (GetFieldIdFunction)native[DARWIN_ART_JNI_SLOT_GetMethodID];
  GetFieldIdFunction get_static_method_id =
      (GetFieldIdFunction)native[DARWIN_ART_JNI_SLOT_GetStaticMethodID];
  NewStringFunction new_string =
      (NewStringFunction)native[DARWIN_ART_JNI_SLOT_NewString];
  GetStringLengthFunction get_string_length =
      (GetStringLengthFunction)native[DARWIN_ART_JNI_SLOT_GetStringLength];
  GetStringCharsFunction get_string_chars =
      (GetStringCharsFunction)native[DARWIN_ART_JNI_SLOT_GetStringChars];
  ReleaseStringCharsFunction release_string_chars =
      (ReleaseStringCharsFunction)native[DARWIN_ART_JNI_SLOT_ReleaseStringChars];
  NewStringUtfFunction new_string_utf =
      (NewStringUtfFunction)native[DARWIN_ART_JNI_SLOT_NewStringUTF];
  GetStringUtfLengthFunction get_string_utf_length =
      (GetStringUtfLengthFunction)
          native[DARWIN_ART_JNI_SLOT_GetStringUTFLength];
  GetStringUtfCharsFunction get_string_utf_chars =
      (GetStringUtfCharsFunction)native[DARWIN_ART_JNI_SLOT_GetStringUTFChars];
  ReleaseStringUtfCharsFunction release_string_utf_chars =
      (ReleaseStringUtfCharsFunction)
          native[DARWIN_ART_JNI_SLOT_ReleaseStringUTFChars];
  GetArrayLengthFunction get_array_length =
      (GetArrayLengthFunction)native[DARWIN_ART_JNI_SLOT_GetArrayLength];
  NewObjectArrayFunction new_object_array =
      (NewObjectArrayFunction)native[DARWIN_ART_JNI_SLOT_NewObjectArray];
  GetObjectArrayElementFunction get_object_array_element =
      (GetObjectArrayElementFunction)
          native[DARWIN_ART_JNI_SLOT_GetObjectArrayElement];
  SetObjectArrayElementFunction set_object_array_element =
      (SetObjectArrayElementFunction)
          native[DARWIN_ART_JNI_SLOT_SetObjectArrayElement];
  NewByteArrayFunction new_byte_array =
      (NewByteArrayFunction)native[DARWIN_ART_JNI_SLOT_NewByteArray];
  GetByteArrayElementsFunction get_byte_array_elements =
      (GetByteArrayElementsFunction)
          native[DARWIN_ART_JNI_SLOT_GetByteArrayElements];
  ReleaseByteArrayElementsFunction release_byte_array_elements =
      (ReleaseByteArrayElementsFunction)
          native[DARWIN_ART_JNI_SLOT_ReleaseByteArrayElements];
  GetByteArrayRegionFunction get_byte_array_region =
      (GetByteArrayRegionFunction)
          native[DARWIN_ART_JNI_SLOT_GetByteArrayRegion];
  SetByteArrayRegionFunction set_byte_array_region =
      (SetByteArrayRegionFunction)
          native[DARWIN_ART_JNI_SLOT_SetByteArrayRegion];

  CHECK(get_version(env) == DARWIN_ART_JNI_VERSION_1_6);
  void* returned_vm = NULL;
  CHECK(get_java_vm(env, &returned_vm) == DARWIN_ART_JNI_OK);
  CHECK(returned_vm == vm);
  CHECK(monitor_enter(env, (void*)(uintptr_t)1) == DARWIN_ART_JNI_ERR);
  CHECK(monitor_exit(env, (void*)(uintptr_t)1) == DARWIN_ART_JNI_ERR);
  CHECK(exception_check(env) == 0);
  CHECK(push_local_frame(env, 16) == DARWIN_ART_JNI_ERR);
  CHECK(pop_local_frame(env, NULL) == NULL);
  const uint16_t utf16[] = {'o', 'k'};
  CHECK(new_string(env, utf16, 2) == NULL);
  CHECK(get_string_length(env, (void*)(uintptr_t)1) == 0);
  CHECK(get_string_chars(env, (void*)(uintptr_t)1, NULL) == NULL);
  release_string_chars(env, (void*)(uintptr_t)1, utf16);
  CHECK(new_string_utf(env, "unavailable") == NULL);
  CHECK(get_string_utf_length(env, (void*)(uintptr_t)1) == 0);
  CHECK(get_string_utf_chars(env, (void*)(uintptr_t)1, NULL) == NULL);
  release_string_utf_chars(env, (void*)(uintptr_t)1, "ignored");
  CHECK(new_global_ref(env, (void*)(uintptr_t)1) == NULL);
  delete_global_ref(env, (void*)(uintptr_t)1);
  CHECK(new_local_ref(env, (void*)(uintptr_t)1) == NULL);
  CHECK(get_object_class(env, (void*)(uintptr_t)1) == NULL);
  CHECK(get_superclass(env, (void*)(uintptr_t)1) == NULL);
  CHECK(get_field_id(env, (void*)(uintptr_t)1, "field", "I") == NULL);
  CHECK(get_method_id(env, (void*)(uintptr_t)1, "method", "()V") == NULL);
  CHECK(get_static_method_id(env, (void*)(uintptr_t)1, "staticMethod", "()V") ==
        NULL);
  CHECK(is_assignable_from(env, (void*)(uintptr_t)1,
                           (void*)(uintptr_t)2) == 0);
  delete_local_ref(env, (void*)(uintptr_t)1);
  CHECK(new_byte_array(env, 4) == NULL);
  CHECK(get_byte_array_elements(env, (void*)(uintptr_t)1, NULL) == NULL);
  release_byte_array_elements(env, (void*)(uintptr_t)1, NULL, 0);
  CHECK(new_object_array(env, 4, (void*)(uintptr_t)1, NULL) == NULL);
  CHECK(get_object_array_element(env, (void*)(uintptr_t)1, 0) == NULL);
  set_object_array_element(env, (void*)(uintptr_t)1, 0,
                           (void*)(uintptr_t)2);
  CHECK(get_array_length(env, (void*)(uintptr_t)1) == 0);
  int8_t region[2] = {1, 2};
  set_byte_array_region(env, (void*)(uintptr_t)1, 0, 2, region);
  get_byte_array_region(env, (void*)(uintptr_t)1, 0, 2, region);
  void* bridge = find_class(env, "fixture/Bridge");
  CHECK(bridge != NULL);
  const DarwinArtJniNativeMethod method = {
      .name = "nativePing",
      .signature = "()I",
      .function = (void*)(uintptr_t)1,
  };
  CHECK(register_natives(env, bridge, &method, 1) == DARWIN_ART_JNI_OK);
  void* exception = find_class(env, "java/lang/RuntimeException");
  CHECK(exception != NULL);
  CHECK(throw_new(env, exception, "proxy-fixture") == DARWIN_ART_JNI_OK);
  CHECK(exception_check(env) == 1);
  CHECK(exception_occurred(env) == NULL);
  exception_clear(env);
  CHECK(exception_check(env) == 0);
  CHECK(darwin_art_jni_fixture_passed() == 1);
  puts("jni proxy native smoke: PASS");
  return 0;
}
