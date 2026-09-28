#include "aim_jni_proxy.h"
#include "jni_slots.h"

#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef void (*RawJniSlot)(void);

typedef struct ProxyEnvHandle {
  const RawJniSlot* functions;
  struct AimJniProxy* owner;
} ProxyEnvHandle;

typedef struct ProxyVmHandle {
  const RawJniSlot* functions;
  struct AimJniProxy* owner;
} ProxyVmHandle;

typedef struct HostEnvHandle {
  const RawJniSlot* functions;
} HostEnvHandle;

typedef struct AndroidArm64VaList {
  void* stack;
  void* gr_top;
  void* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
} AndroidArm64VaList;

struct AimJniProxy {
  uint64_t magic;
  AimJniBackend backend;
  ProxyEnvHandle env;
  ProxyVmHandle vm;
  uint8_t exception_pending;
};

static const uint64_t kProxyMagic = UINT64_C(0x4a4e4950524f5859);

_Static_assert(sizeof(void*) == 8,
               "Android 16 arm64 JNI requires 64-bit pointers");
_Static_assert(sizeof(RawJniSlot) == 8,
               "Android 16 arm64 JNI requires 64-bit function pointers");
_Static_assert(sizeof(int32_t) == 4, "JNI jint is 32-bit");
_Static_assert(sizeof(int8_t) == 1, "JNI jbyte is 8-bit");
_Static_assert(sizeof(AimJniNativeMethod) == 24,
               "JNINativeMethod must remain three pointers");
_Static_assert(_Alignof(AimJniNativeMethod) == 8,
               "JNINativeMethod arm64 alignment drift");
_Static_assert(sizeof(struct AimJniProxy) <=
                   AIM_JNI_PROXY_STORAGE_SIZE,
               "public proxy storage is too small");
_Static_assert(_Alignof(struct AimJniProxy) <=
                   AIM_JNI_PROXY_STORAGE_ALIGNMENT,
               "public proxy storage alignment is too small");

static struct AimJniProxy* EnvOwner(void* raw_env) {
  ProxyEnvHandle* env = (ProxyEnvHandle*)raw_env;
  if (env == NULL || env->owner == NULL || env->owner->magic != kProxyMagic)
    return NULL;
  return env->owner;
}

static struct AimJniProxy* VmOwner(void* raw_vm) {
  ProxyVmHandle* vm = (ProxyVmHandle*)raw_vm;
  if (vm == NULL || vm->owner == NULL || vm->owner->magic != kProxyMagic)
    return NULL;
  return vm->owner;
}

static void* HostEnv(struct AimJniProxy* proxy) {
  if (proxy == NULL || proxy->backend.current_env == NULL) return NULL;
  return proxy->backend.current_env(proxy->backend.context);
}

static RawJniSlot HostSlot(void* raw_host_env, uint32_t slot) {
  HostEnvHandle* env = (HostEnvHandle*)raw_host_env;
  if (env == NULL || env->functions == NULL ||
      slot >= AIM_JNI_NATIVE_SLOT_COUNT)
    return NULL;
  return env->functions[slot];
}

static int32_t ProxyGetVersion(void* raw_env) {
  return EnvOwner(raw_env) == NULL ? 0 : AIM_JNI_VERSION_1_6;
}

static void* ProxyFindClass(void* raw_env, const char* name) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || name == NULL) return NULL;
  // Android media clients use DynamicsProcessing as an optional hardware
  // audio-effect capability and fall back to software gain when it is absent.
  // The detached host does not yet publish libaudioeffect_jni, so do not let
  // resolving this optional class initialize AudioEffect and poison the core
  // AudioTrack JNI bootstrap with an UnsatisfiedLinkError.
  if (strcmp(name, "android/media/audiofx/DynamicsProcessing") == 0) {
    return NULL;
  }
  return proxy->backend.find_class(proxy->backend.context, name);
}

static void* ProxyFromReflectedMethod(void* raw_env, void* method) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_FromReflectedMethod);
  if (raw == NULL || method == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, method);
}

static void* ProxyFromReflectedField(void* raw_env, void* field) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_FromReflectedField);
  if (raw == NULL || field == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, field);
}

static void* ProxyToReflectedMethod(void* raw_env, void* clazz, void* method,
                                    uint8_t is_static) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ToReflectedMethod);
  if (raw == NULL || clazz == NULL || method == NULL) return NULL;
  return ((void* (*)(void*, void*, void*, uint8_t))raw)(host_env, clazz, method,
                                                        is_static);
}

static void* ProxyToReflectedField(void* raw_env, void* clazz, void* field,
                                   uint8_t is_static) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ToReflectedField);
  if (raw == NULL || clazz == NULL || field == NULL) return NULL;
  return ((void* (*)(void*, void*, void*, uint8_t))raw)(host_env, clazz, field,
                                                        is_static);
}

static void* ProxyGetSuperclass(void* raw_env, void* clazz) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetSuperclass);
  if (raw == NULL || clazz == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, clazz);
}

static uint8_t ProxyIsAssignableFrom(void* raw_env, void* clazz1,
                                     void* clazz2) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_IsAssignableFrom);
  if (raw == NULL || clazz1 == NULL || clazz2 == NULL) return 0;
  return ((uint8_t (*)(void*, void*, void*))raw)(host_env, clazz1, clazz2);
}

static int32_t ProxyThrow(void* raw_env, void* throwable) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || throwable == NULL) return AIM_JNI_ERR;
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_Throw);
  if (raw == NULL) return AIM_JNI_ERR;
  const int32_t result = ((int32_t (*)(void*, void*))raw)(host_env, throwable);
  if (result == AIM_JNI_OK) proxy->exception_pending = 1;
  return result;
}

static int32_t ProxyThrowNew(void* raw_env, void* clazz, const char* message) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || clazz == NULL || message == NULL)
    return AIM_JNI_ERR;
  const int32_t result =
      proxy->backend.throw_new(proxy->backend.context, clazz, message);
  if (result == AIM_JNI_OK) proxy->exception_pending = 1;
  return result;
}

static void* ProxyExceptionOccurred(void* raw_env) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ExceptionOccurred);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*))raw)(host_env);
}

static void ProxyExceptionClear(void* raw_env) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL) return;
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ExceptionClear);
  if (raw != NULL) ((void (*)(void*))raw)(host_env);
  proxy->exception_pending = 0;
}

static void ProxyExceptionDescribe(void* raw_env) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ExceptionDescribe);
  if (raw != NULL) ((void (*)(void*))raw)(host_env);
}

/* JNI FatalError is a terminal VM operation.  Forward it to the real ART
 * environment so guest loader failures retain their diagnostic and process
 * boundary, instead of dereferencing an unpopulated proxy slot. */
static void ProxyFatalError(void* raw_env, const char* message) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_FatalError);
  if (raw != NULL) {
    ((void (*)(void*, const char*))raw)(host_env,
                                        message == NULL ? "JNI FatalError" : message);
  }
  fprintf(stderr, "DARWIN JNI FatalError: %s\n",
          message == NULL ? "JNI FatalError" : message);
  abort();
}

static int32_t ProxyPushLocalFrame(void* raw_env, int32_t capacity) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_PushLocalFrame);
  if (raw == NULL) return AIM_JNI_ERR;
  return ((int32_t(*)(void*, int32_t))raw)(host_env, capacity);
}

static void* ProxyPopLocalFrame(void* raw_env, void* result) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_PopLocalFrame);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, result);
}

static int32_t ProxyRegisterNatives(void* raw_env, void* clazz,
                                    const AimJniNativeMethod* methods,
                                    int32_t count) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || clazz == NULL || methods == NULL || count < 0)
    return AIM_JNI_ERR;
  return proxy->backend.register_natives(proxy->backend.context, clazz, methods,
                                         count);
}

static int32_t ProxyUnregisterNatives(void* raw_env, void* clazz) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_UnregisterNatives);
  if (raw == NULL || clazz == NULL) return AIM_JNI_ERR;
  return ((int32_t(*)(void*, void*))raw)(host_env, clazz);
}

static int32_t ProxyMonitorEnter(void* raw_env, void* object) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_MonitorEnter);
  if (raw == NULL || object == NULL) return AIM_JNI_ERR;
  return ((int32_t(*)(void*, void*))raw)(host_env, object);
}

static int32_t ProxyMonitorExit(void* raw_env, void* object) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_MonitorExit);
  if (raw == NULL || object == NULL) return AIM_JNI_ERR;
  return ((int32_t(*)(void*, void*))raw)(host_env, object);
}

static int32_t ProxyGetJavaVM(void* raw_env, void** vm) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || vm == NULL) return AIM_JNI_ERR;
  *vm = &proxy->vm;
  return AIM_JNI_OK;
}

static void ProxyGetStringRegion(void* raw_env, void* string, int32_t start,
                                 int32_t length, uint16_t* output) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStringRegion);
  if (raw != NULL) {
    ((void (*)(void*, void*, int32_t, int32_t, uint16_t*))raw)(
        host_env, string, start, length, output);
  }
}

static void ProxyGetStringUTFRegion(void* raw_env, void* string, int32_t start,
                                    int32_t length, char* output) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStringUTFRegion);
  if (raw != NULL) {
    ((void (*)(void*, void*, int32_t, int32_t, char*))raw)(
        host_env, string, start, length, output);
  }
}

static void* ProxyGetPrimitiveArrayCritical(void* raw_env, void* array,
                                            uint8_t* is_copy) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_GetPrimitiveArrayCritical);
  return raw == NULL
             ? NULL
             : ((void* (*)(void*, void*, uint8_t*))raw)(host_env, array,
                                                         is_copy);
}

static void ProxyReleasePrimitiveArrayCritical(void* raw_env, void* array,
                                               void* elements, int32_t mode) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_ReleasePrimitiveArrayCritical);
  if (raw != NULL) {
    ((void (*)(void*, void*, void*, int32_t))raw)(host_env, array, elements,
                                                  mode);
  }
}

static const uint16_t* ProxyGetStringCritical(void* raw_env, void* string,
                                              uint8_t* is_copy) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStringCritical);
  return raw == NULL
             ? NULL
             : ((const uint16_t* (*)(void*, void*, uint8_t*))raw)(
                   host_env, string, is_copy);
}

static void ProxyReleaseStringCritical(void* raw_env, void* string,
                                       const uint16_t* characters) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ReleaseStringCritical);
  if (raw != NULL) {
    ((void (*)(void*, void*, const uint16_t*))raw)(host_env, string,
                                                   characters);
  }
}

static void* ProxyNewGlobalRef(void* raw_env, void* reference) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewGlobalRef);
  if (raw == NULL) return NULL;
  void* result = ((void* (*)(void*, void*))raw)(host_env, reference);
  if (getenv("AIM_DEBUG_JNI_GLOBAL_REFS") != NULL) {
    fprintf(stderr,
            "ART Android JNI proxy: NewGlobalRef reference=%p result=%p\n",
            reference, result);
  }
  return result;
}

static void ProxyDeleteGlobalRef(void* raw_env, void* reference) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_DeleteGlobalRef);
  if (raw == NULL) return;
  ((void (*)(void*, void*))raw)(host_env, reference);
}

static void* ProxyNewWeakGlobalRef(void* raw_env, void* reference) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewWeakGlobalRef);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, reference);
}

static void ProxyDeleteWeakGlobalRef(void* raw_env, void* reference) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_DeleteWeakGlobalRef);
  if (raw != NULL) ((void (*)(void*, void*))raw)(host_env, reference);
}

static void ProxyDeleteLocalRef(void* raw_env, void* reference) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_DeleteLocalRef);
  if (raw == NULL) return;
  ((void (*)(void*, void*))raw)(host_env, reference);
}

static void* ProxyNewDirectByteBuffer(void* raw_env, void* address,
                                      int64_t capacity) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewDirectByteBuffer);
  if (raw == NULL || address == NULL || capacity < 0) return NULL;
  return ((void* (*)(void*, void*, int64_t))raw)(host_env, address, capacity);
}

static void* ProxyGetDirectBufferAddress(void* raw_env, void* buffer) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetDirectBufferAddress);
  if (raw == NULL) return NULL;
  void* address = ((void* (*)(void*, void*))raw)(host_env, buffer);
  if (getenv("AIM_DEBUG_MEDIA_CODEC") != NULL) {
    fprintf(stderr, "DARWIN JNI direct buffer object=%p address=%p caller=%p\n",
            buffer, address, __builtin_return_address(0));
  }
  return address;
}

static int64_t ProxyGetDirectBufferCapacity(void* raw_env, void* buffer) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetDirectBufferCapacity);
  if (raw == NULL) return -1;
  return ((int64_t (*)(void*, void*))raw)(host_env, buffer);
}

static void* ProxyNewLocalRef(void* raw_env, void* reference) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewLocalRef);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, reference);
}

static uint8_t ProxyIsSameObject(void* raw_env, void* first, void* second) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_IsSameObject);
  if (raw == NULL) return first == second;
  return ((uint8_t(*)(void*, void*, void*))raw)(host_env, first, second);
}

static void* ProxyGetObjectClass(void* raw_env, void* object) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetObjectClass);
  if (raw == NULL || object == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, object);
}

static uint8_t ProxyIsInstanceOf(void* raw_env, void* object, void* clazz) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_IsInstanceOf);
  if (raw == NULL || clazz == NULL) return 0;
  return ((uint8_t(*)(void*, void*, void*))raw)(host_env, object, clazz);
}

static uint64_t ProxyCallMethodVBits(void* raw_env, void* object, void* method,
                                     va_list arguments, int32_t return_shorty) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_method_v == NULL || object == NULL ||
      method == NULL)
    return 0;
  return proxy->backend.call_method_v(proxy->backend.context, object, method,
                                      (void*)arguments, return_shorty, 0);
}

#define DEFINE_CALL_METHOD_V(Name, CType, Shorty)                              \
  static CType ProxyCall##Name##MethodV(void* raw_env, void* object,           \
                                         void* method, va_list arguments) {     \
    return (CType)ProxyCallMethodVBits(raw_env, object, method, arguments,      \
                                       Shorty);                                 \
  }

DEFINE_CALL_METHOD_V(Object, void*, 'L')
DEFINE_CALL_METHOD_V(Boolean, uint8_t, 'Z')
DEFINE_CALL_METHOD_V(Byte, int8_t, 'B')
DEFINE_CALL_METHOD_V(Char, uint16_t, 'C')
DEFINE_CALL_METHOD_V(Short, int16_t, 'S')
DEFINE_CALL_METHOD_V(Int, int32_t, 'I')
DEFINE_CALL_METHOD_V(Long, int64_t, 'J')

static float ProxyCallFloatMethodV(void* raw_env, void* object, void* method,
                                   va_list arguments) {
  const uint32_t bits = (uint32_t)ProxyCallMethodVBits(
      raw_env, object, method, arguments, 'F');
  float value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

static double ProxyCallDoubleMethodV(void* raw_env, void* object, void* method,
                                     va_list arguments) {
  const uint64_t bits =
      ProxyCallMethodVBits(raw_env, object, method, arguments, 'D');
  double value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

static void ProxyCallVoidMethodV(void* raw_env, void* object, void* method,
                                 va_list arguments) {
  (void)ProxyCallMethodVBits(raw_env, object, method, arguments, 'V');
}

extern void aim_jni_proxy_call_void_method(void* raw_env, void* object,
                                                   void* method, ...);
extern int32_t aim_jni_proxy_call_int_method(void* raw_env, void* object,
                                                    void* method, ...);
extern void* aim_jni_proxy_call_object_method(void* raw_env,
                                                     void* object,
                                                     void* method, ...);
extern void* aim_jni_proxy_new_object(void* raw_env, void* clazz,
                                             void* method, ...);

static uint64_t ProxyCallCaptured(
    void* raw_env, void* object, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack, int32_t return_shorty) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_method_v == NULL || object == NULL ||
      method == NULL)
    return 0;
  AndroidArm64VaList arguments = {
      .stack = caller_stack,
      .gr_top = gp_registers + 64,
      .vr_top = fp_registers + 128,
      .gr_offs = -40,
      .vr_offs = -128,
  };
  return proxy->backend.call_method_v(proxy->backend.context, object, method,
                                      &arguments, return_shorty, 0);
}

void aim_jni_proxy_call_void_method_captured(
    void* raw_env, void* object, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  (void)ProxyCallCaptured(raw_env, object, method, gp_registers, fp_registers,
                          caller_stack, 'V');
}

int32_t aim_jni_proxy_call_int_method_captured(
    void* raw_env, void* object, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  return (int32_t)ProxyCallCaptured(raw_env, object, method, gp_registers,
                                    fp_registers, caller_stack, 'I');
}

void* aim_jni_proxy_call_object_method_captured(
    void* raw_env, void* object, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  return (void*)(uintptr_t)ProxyCallCaptured(
      raw_env, object, method, gp_registers, fp_registers, caller_stack, 'L');
}

void* aim_jni_proxy_new_object_captured(
    void* raw_env, void* clazz, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_method_v == NULL || clazz == NULL ||
      method == NULL)
    return NULL;
  AndroidArm64VaList arguments = {
      .stack = caller_stack,
      .gr_top = gp_registers + 64,
      .vr_top = fp_registers + 128,
      .gr_offs = -40,
      .vr_offs = -128,
  };
  return (void*)(uintptr_t)proxy->backend.call_method_v(
      proxy->backend.context, clazz, method, &arguments, 'L', 2);
}

static void* ProxyNewObjectV(void* raw_env, void* clazz, void* method,
                             va_list arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_method_v == NULL || clazz == NULL ||
      method == NULL)
    return NULL;
  return (void*)(uintptr_t)proxy->backend.call_method_v(
      proxy->backend.context, clazz, method, (void*)arguments, 'L', 2);
}

static void* ProxyNewObjectA(void* raw_env, void* clazz, void* method,
                             const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewObjectA);
  if (raw == NULL || clazz == NULL || method == NULL) return NULL;
  return ((void* (*)(void*, void*, void*, const void*))raw)(
      host_env, clazz, method, arguments);
}

#define DEFINE_CALL_METHOD_A(Name, CType)                                    \
  static CType ProxyCall##Name##MethodA(void* raw_env, void* object,          \
                                         void* method,                        \
                                         const void* arguments) {             \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                     \
    void* host_env = HostEnv(proxy);                                          \
    RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_Call##Name##MethodA); \
    if (raw == NULL || object == NULL || method == NULL) return (CType)0;      \
    return ((CType(*)(void*, void*, void*, const void*))raw)(                 \
        host_env, object, method, arguments);                                 \
  }

DEFINE_CALL_METHOD_A(Object, void*)
DEFINE_CALL_METHOD_A(Boolean, uint8_t)
DEFINE_CALL_METHOD_A(Byte, int8_t)
DEFINE_CALL_METHOD_A(Char, uint16_t)
DEFINE_CALL_METHOD_A(Short, int16_t)
DEFINE_CALL_METHOD_A(Int, int32_t)
DEFINE_CALL_METHOD_A(Long, int64_t)
DEFINE_CALL_METHOD_A(Float, float)
DEFINE_CALL_METHOD_A(Double, double)

static void ProxyCallVoidMethodA(void* raw_env, void* object, void* method,
                                 const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_CallVoidMethodA);
  if (raw != NULL && object != NULL && method != NULL) {
    ((void (*)(void*, void*, void*, const void*))raw)(host_env, object, method,
                                                      arguments);
  }
}

#undef DEFINE_CALL_METHOD_A

#undef DEFINE_CALL_METHOD_V

static uint64_t ProxyCallStaticMethodVBits(void* raw_env, void* clazz,
                                           void* method, va_list arguments,
                                           int32_t return_shorty) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_method_v == NULL || clazz == NULL ||
      method == NULL)
    return 0;
  return proxy->backend.call_method_v(proxy->backend.context, clazz, method,
                                      (void*)arguments, return_shorty, 1);
}

#define DEFINE_CALL_STATIC_METHOD_V(Name, CType, Shorty)                       \
  static CType ProxyCallStatic##Name##MethodV(                                \
      void* raw_env, void* clazz, void* method, va_list arguments) {           \
    return (CType)ProxyCallStaticMethodVBits(raw_env, clazz, method, arguments,\
                                              Shorty);                          \
  }

DEFINE_CALL_STATIC_METHOD_V(Object, void*, 'L')
DEFINE_CALL_STATIC_METHOD_V(Boolean, uint8_t, 'Z')
DEFINE_CALL_STATIC_METHOD_V(Byte, int8_t, 'B')
DEFINE_CALL_STATIC_METHOD_V(Char, uint16_t, 'C')
DEFINE_CALL_STATIC_METHOD_V(Short, int16_t, 'S')
DEFINE_CALL_STATIC_METHOD_V(Int, int32_t, 'I')
DEFINE_CALL_STATIC_METHOD_V(Long, int64_t, 'J')

static float ProxyCallStaticFloatMethodV(void* raw_env, void* clazz,
                                         void* method, va_list arguments) {
  const uint32_t bits = (uint32_t)ProxyCallStaticMethodVBits(
      raw_env, clazz, method, arguments, 'F');
  float value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

static double ProxyCallStaticDoubleMethodV(void* raw_env, void* clazz,
                                           void* method, va_list arguments) {
  const uint64_t bits =
      ProxyCallStaticMethodVBits(raw_env, clazz, method, arguments, 'D');
  double value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

static void ProxyCallStaticVoidMethodV(void* raw_env, void* clazz, void* method,
                                       va_list arguments) {
  (void)ProxyCallStaticMethodVBits(raw_env, clazz, method, arguments, 'V');
}

extern void aim_jni_proxy_call_static_void_method(
    void* raw_env, void* clazz, void* method, ...);
extern int32_t aim_jni_proxy_call_static_int_method(
    void* raw_env, void* clazz, void* method, ...);
extern void* aim_jni_proxy_call_static_object_method(
    void* raw_env, void* clazz, void* method, ...);

static uint64_t ProxyCallStaticCaptured(
    void* raw_env, void* clazz, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack, int32_t return_shorty) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_method_v == NULL || clazz == NULL ||
      method == NULL)
    return 0;
  AndroidArm64VaList arguments = {
      .stack = caller_stack,
      .gr_top = gp_registers + 64,
      .vr_top = fp_registers + 128,
      .gr_offs = -40,
      .vr_offs = -128,
  };
  return proxy->backend.call_method_v(proxy->backend.context, clazz, method,
                                      &arguments, return_shorty, 1);
}

void aim_jni_proxy_call_static_void_method_captured(
    void* raw_env, void* clazz, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  (void)ProxyCallStaticCaptured(raw_env, clazz, method, gp_registers,
                                fp_registers, caller_stack, 'V');
}

int32_t aim_jni_proxy_call_static_int_method_captured(
    void* raw_env, void* clazz, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  return (int32_t)ProxyCallStaticCaptured(raw_env, clazz, method, gp_registers,
                                          fp_registers, caller_stack, 'I');
}

void* aim_jni_proxy_call_static_object_method_captured(
    void* raw_env, void* clazz, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  return (void*)(uintptr_t)ProxyCallStaticCaptured(
      raw_env, clazz, method, gp_registers, fp_registers, caller_stack, 'L');
}

#define DEFINE_CALL_STATIC_METHOD_A(Name, CType)                             \
  static CType ProxyCallStatic##Name##MethodA(                              \
      void* raw_env, void* clazz, void* method, const void* arguments) {     \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                    \
    void* host_env = HostEnv(proxy);                                         \
    RawJniSlot raw =                                                        \
        HostSlot(host_env, AIM_JNI_SLOT_CallStatic##Name##MethodA);   \
    if (raw == NULL || clazz == NULL || method == NULL) return (CType)0;     \
    return ((CType(*)(void*, void*, void*, const void*))raw)(                \
        host_env, clazz, method, arguments);                                 \
  }

DEFINE_CALL_STATIC_METHOD_A(Object, void*)
DEFINE_CALL_STATIC_METHOD_A(Boolean, uint8_t)
DEFINE_CALL_STATIC_METHOD_A(Byte, int8_t)
DEFINE_CALL_STATIC_METHOD_A(Char, uint16_t)
DEFINE_CALL_STATIC_METHOD_A(Short, int16_t)
DEFINE_CALL_STATIC_METHOD_A(Int, int32_t)
DEFINE_CALL_STATIC_METHOD_A(Long, int64_t)
DEFINE_CALL_STATIC_METHOD_A(Float, float)
DEFINE_CALL_STATIC_METHOD_A(Double, double)

static void ProxyCallStaticVoidMethodA(void* raw_env, void* clazz,
                                       void* method, const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallStaticVoidMethodA);
  if (raw != NULL && clazz != NULL && method != NULL) {
    ((void (*)(void*, void*, void*, const void*))raw)(host_env, clazz, method,
                                                      arguments);
  }
}

#undef DEFINE_CALL_STATIC_METHOD_A

#undef DEFINE_CALL_STATIC_METHOD_V

static void* ProxyGetFieldID(void* raw_env, void* clazz, const char* name,
                             const char* signature) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetFieldID);
  if (raw == NULL || clazz == NULL || name == NULL || signature == NULL)
    return NULL;
  void* field = ((void* (*)(void*, void*, const char*, const char*))raw)(
      host_env, clazz, name, signature);
  if (getenv("AIM_DEBUG_JNI_CALLS") != NULL) {
    fprintf(stderr, "DARWIN JNI field name=%s signature=%s id=%p\n", name,
            signature, field);
  }
  return field;
}

#define DEFINE_GET_FIELD(Name, CType)                                         \
  static CType ProxyGet##Name##Field(void* raw_env, void* object,             \
                                      void* field) {                           \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                      \
    void* host_env = HostEnv(proxy);                                          \
    RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_Get##Name##Field);\
    if (raw == NULL || object == NULL || field == NULL) return (CType)0;       \
    return ((CType(*)(void*, void*, void*))raw)(host_env, object, field);      \
  }

DEFINE_GET_FIELD(Object, void*)
DEFINE_GET_FIELD(Boolean, uint8_t)
DEFINE_GET_FIELD(Byte, int8_t)
DEFINE_GET_FIELD(Char, uint16_t)
DEFINE_GET_FIELD(Short, int16_t)
DEFINE_GET_FIELD(Int, int32_t)
DEFINE_GET_FIELD(Long, int64_t)
DEFINE_GET_FIELD(Float, float)
DEFINE_GET_FIELD(Double, double)

#undef DEFINE_GET_FIELD

#define DEFINE_SET_FIELD(Name, CType)                                         \
  static void ProxySet##Name##Field(void* raw_env, void* object, void* field, \
                                     CType value) {                            \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                      \
    void* host_env = HostEnv(proxy);                                          \
    RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_Set##Name##Field);\
    if (raw == NULL || object == NULL || field == NULL) return;                \
    ((void (*)(void*, void*, void*, CType))raw)(host_env, object, field,       \
                                                 value);                       \
  }

DEFINE_SET_FIELD(Object, void*)
DEFINE_SET_FIELD(Boolean, uint8_t)
DEFINE_SET_FIELD(Byte, int8_t)
DEFINE_SET_FIELD(Char, uint16_t)
DEFINE_SET_FIELD(Short, int16_t)
DEFINE_SET_FIELD(Int, int32_t)
DEFINE_SET_FIELD(Long, int64_t)
DEFINE_SET_FIELD(Float, float)
DEFINE_SET_FIELD(Double, double)

#undef DEFINE_SET_FIELD

static void* ProxyGetMethodID(void* raw_env, void* clazz, const char* name,
                              const char* signature) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy != NULL && proxy->backend.get_method_id != NULL) {
    return proxy->backend.get_method_id(proxy->backend.context, clazz, name,
                                        signature, 0);
  }
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetMethodID);
  if (raw == NULL || clazz == NULL || name == NULL || signature == NULL)
    return NULL;
  return ((void* (*)(void*, void*, const char*, const char*))raw)(
      host_env, clazz, name, signature);
}

static void* ProxyGetStaticMethodID(void* raw_env, void* clazz,
                                    const char* name, const char* signature) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy != NULL && proxy->backend.get_method_id != NULL) {
    return proxy->backend.get_method_id(proxy->backend.context, clazz, name,
                                        signature, 1);
  }
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStaticMethodID);
  if (raw == NULL || clazz == NULL || name == NULL || signature == NULL)
    return NULL;
  return ((void* (*)(void*, void*, const char*, const char*))raw)(
      host_env, clazz, name, signature);
}

static void* ProxyGetStaticFieldID(void* raw_env, void* clazz,
                                   const char* name, const char* signature) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStaticFieldID);
  if (raw == NULL || clazz == NULL || name == NULL || signature == NULL)
    return NULL;
  return ((void* (*)(void*, void*, const char*, const char*))raw)(
      host_env, clazz, name, signature);
}

#define DEFINE_GET_STATIC_FIELD(Name, CType)                                  \
  static CType ProxyGetStatic##Name##Field(void* raw_env, void* clazz,        \
                                            void* field) {                     \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                      \
    void* host_env = HostEnv(proxy);                                          \
    RawJniSlot raw =                                                         \
        HostSlot(host_env, AIM_JNI_SLOT_GetStatic##Name##Field);       \
    if (raw == NULL || clazz == NULL || field == NULL) return (CType)0;        \
    return ((CType(*)(void*, void*, void*))raw)(host_env, clazz, field);       \
  }

DEFINE_GET_STATIC_FIELD(Object, void*)
DEFINE_GET_STATIC_FIELD(Boolean, uint8_t)
DEFINE_GET_STATIC_FIELD(Byte, int8_t)
DEFINE_GET_STATIC_FIELD(Char, uint16_t)
DEFINE_GET_STATIC_FIELD(Short, int16_t)
DEFINE_GET_STATIC_FIELD(Int, int32_t)
DEFINE_GET_STATIC_FIELD(Long, int64_t)
DEFINE_GET_STATIC_FIELD(Float, float)
DEFINE_GET_STATIC_FIELD(Double, double)

#undef DEFINE_GET_STATIC_FIELD

#define DEFINE_SET_STATIC_FIELD(Name, CType)                                  \
  static void ProxySetStatic##Name##Field(void* raw_env, void* clazz,         \
                                           void* field, CType value) {         \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                      \
    void* host_env = HostEnv(proxy);                                          \
    RawJniSlot raw =                                                         \
        HostSlot(host_env, AIM_JNI_SLOT_SetStatic##Name##Field);       \
    if (raw == NULL || clazz == NULL || field == NULL) return;                 \
    ((void (*)(void*, void*, void*, CType))raw)(host_env, clazz, field, value);\
  }

DEFINE_SET_STATIC_FIELD(Object, void*)
DEFINE_SET_STATIC_FIELD(Boolean, uint8_t)
DEFINE_SET_STATIC_FIELD(Byte, int8_t)
DEFINE_SET_STATIC_FIELD(Char, uint16_t)
DEFINE_SET_STATIC_FIELD(Short, int16_t)
DEFINE_SET_STATIC_FIELD(Int, int32_t)
DEFINE_SET_STATIC_FIELD(Long, int64_t)
DEFINE_SET_STATIC_FIELD(Float, float)
DEFINE_SET_STATIC_FIELD(Double, double)

#undef DEFINE_SET_STATIC_FIELD

static void* ProxyNewString(void* raw_env, const uint16_t* characters,
                            int32_t length) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewString);
  // Android ART accepts NewString(nullptr, 0), which jni_zero uses while
  // initializing its process-wide empty-string singleton. A null character
  // buffer is invalid only when there are characters to read.
  if (raw == NULL || length < 0 || (characters == NULL && length != 0)) {
    return NULL;
  }
  return ((void* (*)(void*, const uint16_t*, int32_t))raw)(host_env,
                                                           characters, length);
}

static int32_t ProxyGetStringLength(void* raw_env, void* string) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStringLength);
  if (raw == NULL || string == NULL) return 0;
  return ((int32_t (*)(void*, void*))raw)(host_env, string);
}

static const uint16_t* ProxyGetStringChars(void* raw_env, void* string,
                                            uint8_t* is_copy) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStringChars);
  if (raw == NULL || string == NULL) return NULL;
  return ((const uint16_t* (*)(void*, void*, uint8_t*))raw)(host_env, string,
                                                            is_copy);
}

static void ProxyReleaseStringChars(void* raw_env, void* string,
                                     const uint16_t* characters) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ReleaseStringChars);
  if (raw == NULL || string == NULL || characters == NULL) return;
  ((void (*)(void*, void*, const uint16_t*))raw)(host_env, string, characters);
}

static void* ProxyNewStringUTF(void* raw_env, const char* bytes) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewStringUTF);
  if (raw == NULL || bytes == NULL) return NULL;
  return ((void* (*)(void*, const char*))raw)(host_env, bytes);
}

static int32_t ProxyGetStringUTFLength(void* raw_env, void* string) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStringUTFLength);
  if (raw == NULL || string == NULL) return 0;
  return ((int32_t (*)(void*, void*))raw)(host_env, string);
}

static const char* ProxyGetStringUTFChars(void* raw_env, void* string,
                                          uint8_t* is_copy) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetStringUTFChars);
  if (raw == NULL || string == NULL) return NULL;
  return ((const char* (*)(void*, void*, uint8_t*))raw)(host_env, string,
                                                        is_copy);
}

static void ProxyReleaseStringUTFChars(void* raw_env, void* string,
                                       const char* bytes) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_ReleaseStringUTFChars);
  if (raw == NULL || string == NULL || bytes == NULL) return;
  ((void (*)(void*, void*, const char*))raw)(host_env, string, bytes);
}

static int32_t ProxyGetArrayLength(void* raw_env, void* array) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetArrayLength);
  if (raw == NULL) return 0;
  return ((int32_t (*)(void*, void*))raw)(host_env, array);
}

static void* ProxyNewObjectArray(void* raw_env, int32_t length, void* clazz,
                                 void* initial) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewObjectArray);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*, int32_t, void*, void*))raw)(host_env, length,
                                                        clazz, initial);
}

static void* ProxyGetObjectArrayElement(void* raw_env, void* array,
                                        int32_t index) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_GetObjectArrayElement);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*, void*, int32_t))raw)(host_env, array, index);
}

static void ProxySetObjectArrayElement(void* raw_env, void* array,
                                       int32_t index, void* value) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_SetObjectArrayElement);
  if (raw == NULL) return;
  ((void (*)(void*, void*, int32_t, void*))raw)(host_env, array, index, value);
}

static void* ProxyNewByteArray(void* raw_env, int32_t length) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_NewByteArray);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*, int32_t))raw)(host_env, length);
}

#define DEFINE_PRIMITIVE_ARRAY_PROXY(Name, CType)                              \
  static void* ProxyNew##Name##Array(void* raw_env, int32_t length) {          \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                       \
    void* host_env = HostEnv(proxy);                                           \
    RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_New##Name##Array); \
    if (raw == NULL) return NULL;                                              \
    return ((void* (*)(void*, int32_t))raw)(host_env, length);                 \
  }                                                                            \
  static CType* ProxyGet##Name##ArrayElements(void* raw_env, void* array,      \
                                               uint8_t* is_copy) {              \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                       \
    void* host_env = HostEnv(proxy);                                           \
    RawJniSlot raw =                                                          \
        HostSlot(host_env, AIM_JNI_SLOT_Get##Name##ArrayElements);      \
    if (raw == NULL) return NULL;                                              \
    return ((CType* (*)(void*, void*, uint8_t*))raw)(host_env, array,          \
                                                      is_copy);                 \
  }                                                                            \
  static void ProxyRelease##Name##ArrayElements(                               \
      void* raw_env, void* array, CType* elements, int32_t mode) {             \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                       \
    void* host_env = HostEnv(proxy);                                           \
    RawJniSlot raw = HostSlot(                                                 \
        host_env, AIM_JNI_SLOT_Release##Name##ArrayElements);           \
    if (raw != NULL)                                                           \
      ((void (*)(void*, void*, CType*, int32_t))raw)(host_env, array,          \
                                                      elements, mode);          \
  }                                                                            \
  static void ProxyGet##Name##ArrayRegion(void* raw_env, void* array,          \
                                           int32_t start, int32_t length,       \
                                           CType* values) {                     \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                       \
    void* host_env = HostEnv(proxy);                                           \
    RawJniSlot raw =                                                          \
        HostSlot(host_env, AIM_JNI_SLOT_Get##Name##ArrayRegion);        \
    if (raw != NULL && (length == 0 || values != NULL))                        \
      ((void (*)(void*, void*, int32_t, int32_t, CType*))raw)(                 \
          host_env, array, start, length, values);                             \
  }                                                                            \
  static void ProxySet##Name##ArrayRegion(                                     \
      void* raw_env, void* array, int32_t start, int32_t length,               \
      const CType* values) {                                                   \
    struct AimJniProxy* proxy = EnvOwner(raw_env);                       \
    void* host_env = HostEnv(proxy);                                           \
    RawJniSlot raw =                                                          \
        HostSlot(host_env, AIM_JNI_SLOT_Set##Name##ArrayRegion);        \
    if (raw != NULL && (length == 0 || values != NULL))                        \
      ((void (*)(void*, void*, int32_t, int32_t, const CType*))raw)(           \
          host_env, array, start, length, values);                             \
  }

DEFINE_PRIMITIVE_ARRAY_PROXY(Boolean, uint8_t)
DEFINE_PRIMITIVE_ARRAY_PROXY(Char, uint16_t)
DEFINE_PRIMITIVE_ARRAY_PROXY(Short, int16_t)
DEFINE_PRIMITIVE_ARRAY_PROXY(Int, int32_t)
DEFINE_PRIMITIVE_ARRAY_PROXY(Long, int64_t)
DEFINE_PRIMITIVE_ARRAY_PROXY(Float, float)
DEFINE_PRIMITIVE_ARRAY_PROXY(Double, double)

#undef DEFINE_PRIMITIVE_ARRAY_PROXY

static int8_t* ProxyGetByteArrayElements(void* raw_env, void* array,
                                         uint8_t* is_copy) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetByteArrayElements);
  if (raw == NULL) return NULL;
  return ((int8_t* (*)(void*, void*, uint8_t*))raw)(host_env, array, is_copy);
}

static void ProxyReleaseByteArrayElements(void* raw_env, void* array,
                                          int8_t* elements, int32_t mode) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_ReleaseByteArrayElements);
  if (raw == NULL) return;
  ((void (*)(void*, void*, int8_t*, int32_t))raw)(host_env, array, elements,
                                                  mode);
}

static void ProxyGetByteArrayRegion(void* raw_env, void* array, int32_t start,
                                    int32_t length, int8_t* bytes) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetByteArrayRegion);
  if (raw == NULL || (length > 0 && bytes == NULL)) return;
  ((void (*)(void*, void*, int32_t, int32_t, int8_t*))raw)(
      host_env, array, start, length, bytes);
}

static void ProxySetByteArrayRegion(void* raw_env, void* array, int32_t start,
                                    int32_t length, const int8_t* bytes) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_SetByteArrayRegion);
  if (raw == NULL || (length > 0 && bytes == NULL)) return;
  ((void (*)(void*, void*, int32_t, int32_t, const int8_t*))raw)(
      host_env, array, start, length, bytes);
}

static uint8_t ProxyExceptionCheck(void* raw_env) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL) return 1;
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_ExceptionCheck);
  if (raw != NULL) {
    const uint8_t pending = ((uint8_t (*)(void*))raw)(host_env);
    if (pending != 0 && getenv("AIM_DEBUG_JNI_EXCEPTION") != NULL) {
      RawJniSlot describe =
          HostSlot(host_env, AIM_JNI_SLOT_ExceptionDescribe);
      if (describe != NULL) ((void (*)(void*))describe)(host_env);
    }
    return pending;
  }
  return proxy->exception_pending;
}

static int32_t ProxyGetEnv(void* raw_vm, void** output, int32_t version) {
  struct AimJniProxy* proxy = VmOwner(raw_vm);
  if (output == NULL) return AIM_JNI_ERR;
  *output = NULL;
  if (proxy == NULL) return AIM_JNI_ERR;
  if (version != AIM_JNI_VERSION_1_1 &&
      version != AIM_JNI_VERSION_1_2 &&
      version != AIM_JNI_VERSION_1_4 &&
      version != AIM_JNI_VERSION_1_6)
    return AIM_JNI_EVERSION;
  if (proxy->backend.current_env != NULL &&
      proxy->backend.current_env(proxy->backend.context) == NULL)
    return AIM_JNI_EDETACHED;
  *output = &proxy->env;
  return AIM_JNI_OK;
}

static int32_t ProxyAttachCurrentThread(void* raw_vm, void** output,
                                        void* arguments) {
  struct AimJniProxy* proxy = VmOwner(raw_vm);
  if (output == NULL) return AIM_JNI_ERR;
  *output = NULL;
  if (proxy == NULL || proxy->backend.attach_current_thread == NULL)
    return AIM_JNI_ERR;
  const int32_t status = proxy->backend.attach_current_thread(
      proxy->backend.context, arguments, 0);
  if (status == AIM_JNI_OK) *output = &proxy->env;
  return status;
}

static int32_t ProxyAttachCurrentThreadAsDaemon(void* raw_vm, void** output,
                                                void* arguments) {
  struct AimJniProxy* proxy = VmOwner(raw_vm);
  if (output == NULL) return AIM_JNI_ERR;
  *output = NULL;
  if (proxy == NULL || proxy->backend.attach_current_thread == NULL)
    return AIM_JNI_ERR;
  const int32_t status = proxy->backend.attach_current_thread(
      proxy->backend.context, arguments, 1);
  if (status == AIM_JNI_OK) *output = &proxy->env;
  return status;
}

static int32_t ProxyDetachCurrentThread(void* raw_vm) {
  struct AimJniProxy* proxy = VmOwner(raw_vm);
  if (proxy == NULL || proxy->backend.detach_current_thread == NULL)
    return AIM_JNI_ERR;
  return proxy->backend.detach_current_thread(proxy->backend.context);
}

/* ---- Remaining JNIEnv slots ------------------------------------------- */

/* Register-only entries: pointer and integer arguments, identical under
 * Android AAPCS64 and Darwin ARM64; forwarded to the current ART env. */
static void* ProxyDefineClass(void* raw_env, const char* name, void* loader,
                              const int8_t* buffer, int32_t length) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_DefineClass);
  if (raw == NULL) return NULL;
  return ((void* (*)(void*, const char*, void*, const int8_t*, int32_t))raw)(
      host_env, name, loader, buffer, length);
}

static int32_t ProxyEnsureLocalCapacity(void* raw_env, int32_t capacity) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_EnsureLocalCapacity);
  if (raw == NULL) return AIM_JNI_ERR;
  return ((int32_t (*)(void*, int32_t))raw)(host_env, capacity);
}

static void* ProxyAllocObject(void* raw_env, void* clazz) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_AllocObject);
  if (raw == NULL || clazz == NULL) return NULL;
  return ((void* (*)(void*, void*))raw)(host_env, clazz);
}

static int32_t ProxyGetObjectRefType(void* raw_env, void* object) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw = HostSlot(host_env, AIM_JNI_SLOT_GetObjectRefType);
  if (raw == NULL) return 0; /* JNIInvalidRefType */
  return ((int32_t (*)(void*, void*))raw)(host_env, object);
}

/* CallNonvirtual<Type>MethodA: jvalue[] is a pointer to 8-byte unions under
 * both ABIs, so the host ...A entry is called directly. */
static void* ProxyCallNonvirtualObjectMethodA(void* raw_env, void* object,
                                              void* clazz, void* method,
                                              const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualObjectMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return NULL;
  return ((void* (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static uint8_t ProxyCallNonvirtualBooleanMethodA(void* raw_env, void* object,
                                                 void* clazz, void* method,
                                                 const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualBooleanMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (uint8_t)0;
  return ((uint8_t (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static int8_t ProxyCallNonvirtualByteMethodA(void* raw_env, void* object,
                                             void* clazz, void* method,
                                             const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualByteMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (int8_t)0;
  return ((int8_t (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static uint16_t ProxyCallNonvirtualCharMethodA(void* raw_env, void* object,
                                               void* clazz, void* method,
                                               const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualCharMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (uint16_t)0;
  return ((uint16_t (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static int16_t ProxyCallNonvirtualShortMethodA(void* raw_env, void* object,
                                               void* clazz, void* method,
                                               const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualShortMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (int16_t)0;
  return ((int16_t (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static int32_t ProxyCallNonvirtualIntMethodA(void* raw_env, void* object,
                                             void* clazz, void* method,
                                             const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualIntMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (int32_t)0;
  return ((int32_t (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static int64_t ProxyCallNonvirtualLongMethodA(void* raw_env, void* object,
                                              void* clazz, void* method,
                                              const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualLongMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (int64_t)0;
  return ((int64_t (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static float ProxyCallNonvirtualFloatMethodA(void* raw_env, void* object,
                                             void* clazz, void* method,
                                             const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualFloatMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (float)0;
  return ((float (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static double ProxyCallNonvirtualDoubleMethodA(void* raw_env, void* object,
                                               void* clazz, void* method,
                                               const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualDoubleMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL)
    return (double)0;
  return ((double (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

static void ProxyCallNonvirtualVoidMethodA(void* raw_env, void* object,
                                           void* clazz, void* method,
                                           const void* arguments) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  void* host_env = HostEnv(proxy);
  RawJniSlot raw =
      HostSlot(host_env, AIM_JNI_SLOT_CallNonvirtualVoidMethodA);
  if (raw == NULL || object == NULL || clazz == NULL || method == NULL) return;
  ((void (*)(void*, void*, void*, void*, const void*))raw)(
      host_env, object, clazz, method, arguments);
}

/* CallNonvirtual<Type>MethodV: Android va_list, translated by the backend. */
static uint64_t ProxyCallNonvirtualBits(void* raw_env, void* object,
                                        void* clazz, void* method,
                                        void* android_va_list,
                                        int32_t return_shorty) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_nonvirtual_method_v == NULL ||
      object == NULL || clazz == NULL || method == NULL)
    return 0;
  return proxy->backend.call_nonvirtual_method_v(proxy->backend.context, object,
                                                 clazz, method, android_va_list,
                                                 return_shorty);
}

static void* ProxyCallNonvirtualObjectMethodV(void* raw_env, void* object,
                                              void* clazz, void* method,
                                              va_list arguments) {
  return (void*)(uintptr_t)(ProxyCallNonvirtualBits(
      raw_env, object, clazz, method, (void*)arguments, 'L'));
}

static uint8_t ProxyCallNonvirtualBooleanMethodV(void* raw_env, void* object,
                                                 void* clazz, void* method,
                                                 va_list arguments) {
  return (uint8_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           (void*)arguments, 'Z'));
}

static int8_t ProxyCallNonvirtualByteMethodV(void* raw_env, void* object,
                                             void* clazz, void* method,
                                             va_list arguments) {
  return (int8_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                          (void*)arguments, 'B'));
}

static uint16_t ProxyCallNonvirtualCharMethodV(void* raw_env, void* object,
                                               void* clazz, void* method,
                                               va_list arguments) {
  return (uint16_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                            (void*)arguments, 'C'));
}

static int16_t ProxyCallNonvirtualShortMethodV(void* raw_env, void* object,
                                               void* clazz, void* method,
                                               va_list arguments) {
  return (int16_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           (void*)arguments, 'S'));
}

static int32_t ProxyCallNonvirtualIntMethodV(void* raw_env, void* object,
                                             void* clazz, void* method,
                                             va_list arguments) {
  return (int32_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           (void*)arguments, 'I'));
}

static int64_t ProxyCallNonvirtualLongMethodV(void* raw_env, void* object,
                                              void* clazz, void* method,
                                              va_list arguments) {
  return (int64_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           (void*)arguments, 'J'));
}

static float ProxyCallNonvirtualFloatMethodV(void* raw_env, void* object,
                                             void* clazz, void* method,
                                             va_list arguments) {
  const uint32_t bits = (uint32_t)(ProxyCallNonvirtualBits(
      raw_env, object, clazz, method, (void*)arguments, 'F'));
  float value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

static double ProxyCallNonvirtualDoubleMethodV(void* raw_env, void* object,
                                               void* clazz, void* method,
                                               va_list arguments) {
  const uint64_t bits = ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                                (void*)arguments, 'D');
  double value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

static void ProxyCallNonvirtualVoidMethodV(void* raw_env, void* object,
                                           void* clazz, void* method,
                                           va_list arguments) {
  (void)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                 (void*)arguments, 'V'));
}

/* Variadic entries: the assembly thunks capture the Android argument banks;
 * these build the Android va_list from them. Three named arguments leave
 * five variadic GP registers (gr_offs -40); four leave four (-32). */
static AndroidArm64VaList CapturedVaList(uint8_t* gp_registers,
                                         uint8_t* fp_registers,
                                         uint8_t* caller_stack,
                                         int32_t named_gp) {
  AndroidArm64VaList arguments = {
      .stack = caller_stack,
      .gr_top = gp_registers + 64,
      .vr_top = fp_registers + 128,
      .gr_offs = -8 * (8 - named_gp),
      .vr_offs = -128,
  };
  return arguments;
}

static uint64_t ProxyCallCapturedList(void* raw_env, void* receiver,
                                      void* method,
                                      AndroidArm64VaList* arguments,
                                      int32_t return_shorty,
                                      int32_t is_static) {
  struct AimJniProxy* proxy = EnvOwner(raw_env);
  if (proxy == NULL || proxy->backend.call_method_v == NULL ||
      receiver == NULL || method == NULL)
    return 0;
  return proxy->backend.call_method_v(proxy->backend.context, receiver, method,
                                      arguments, return_shorty, is_static);
}

extern uint8_t aim_jni_proxy_call_boolean_method(void* raw_env,
                                                        void* receiver,
                                                        void* method, ...);
uint8_t aim_jni_proxy_call_boolean_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (uint8_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                         'Z', 0));
}

extern uint8_t aim_jni_proxy_call_static_boolean_method(void* raw_env,
                                                               void* receiver,
                                                               void* method,
                                                               ...);
uint8_t aim_jni_proxy_call_static_boolean_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (uint8_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                         'Z', 1));
}

extern int8_t aim_jni_proxy_call_byte_method(void* raw_env,
                                                    void* receiver,
                                                    void* method, ...);
int8_t aim_jni_proxy_call_byte_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (int8_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                        'B', 0));
}

extern int8_t aim_jni_proxy_call_static_byte_method(void* raw_env,
                                                           void* receiver,
                                                           void* method, ...);
int8_t aim_jni_proxy_call_static_byte_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (int8_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                        'B', 1));
}

extern uint16_t aim_jni_proxy_call_char_method(void* raw_env,
                                                      void* receiver,
                                                      void* method, ...);
uint16_t aim_jni_proxy_call_char_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (uint16_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                          'C', 0));
}

extern uint16_t aim_jni_proxy_call_static_char_method(void* raw_env,
                                                             void* receiver,
                                                             void* method, ...);
uint16_t aim_jni_proxy_call_static_char_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (uint16_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                          'C', 1));
}

extern int16_t aim_jni_proxy_call_short_method(void* raw_env,
                                                      void* receiver,
                                                      void* method, ...);
int16_t aim_jni_proxy_call_short_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (int16_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                         'S', 0));
}

extern int16_t aim_jni_proxy_call_static_short_method(void* raw_env,
                                                             void* receiver,
                                                             void* method, ...);
int16_t aim_jni_proxy_call_static_short_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (int16_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                         'S', 1));
}

extern int64_t aim_jni_proxy_call_long_method(void* raw_env,
                                                     void* receiver,
                                                     void* method, ...);
int64_t aim_jni_proxy_call_long_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (int64_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                         'J', 0));
}

extern int64_t aim_jni_proxy_call_static_long_method(void* raw_env,
                                                            void* receiver,
                                                            void* method, ...);
int64_t aim_jni_proxy_call_static_long_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  return (int64_t)(ProxyCallCapturedList(raw_env, receiver, method, &arguments,
                                         'J', 1));
}

extern float aim_jni_proxy_call_float_method(void* raw_env,
                                                    void* receiver,
                                                    void* method, ...);
float aim_jni_proxy_call_float_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  const uint32_t bits = (uint32_t)(ProxyCallCapturedList(
      raw_env, receiver, method, &arguments, 'F', 0));
  float value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

extern float aim_jni_proxy_call_static_float_method(void* raw_env,
                                                           void* receiver,
                                                           void* method, ...);
float aim_jni_proxy_call_static_float_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  const uint32_t bits = (uint32_t)(ProxyCallCapturedList(
      raw_env, receiver, method, &arguments, 'F', 1));
  float value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

extern double aim_jni_proxy_call_double_method(void* raw_env,
                                                      void* receiver,
                                                      void* method, ...);
double aim_jni_proxy_call_double_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  const uint64_t bits =
      ProxyCallCapturedList(raw_env, receiver, method, &arguments, 'D', 0);
  double value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

extern double aim_jni_proxy_call_static_double_method(void* raw_env,
                                                             void* receiver,
                                                             void* method, ...);
double aim_jni_proxy_call_static_double_method_captured(
    void* raw_env, void* receiver, void* method, uint8_t* gp_registers,
    uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 3);
  const uint64_t bits =
      ProxyCallCapturedList(raw_env, receiver, method, &arguments, 'D', 1);
  double value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

extern void* aim_jni_proxy_call_nonvirtual_object_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
void* aim_jni_proxy_call_nonvirtual_object_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  return (void*)(uintptr_t)(ProxyCallNonvirtualBits(raw_env, object, clazz,
                                                    method, &arguments, 'L'));
}

extern uint8_t aim_jni_proxy_call_nonvirtual_boolean_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
uint8_t aim_jni_proxy_call_nonvirtual_boolean_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  return (uint8_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           &arguments, 'Z'));
}

extern int8_t aim_jni_proxy_call_nonvirtual_byte_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
int8_t aim_jni_proxy_call_nonvirtual_byte_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  return (int8_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                          &arguments, 'B'));
}

extern uint16_t aim_jni_proxy_call_nonvirtual_char_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
uint16_t aim_jni_proxy_call_nonvirtual_char_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  return (uint16_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                            &arguments, 'C'));
}

extern int16_t aim_jni_proxy_call_nonvirtual_short_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
int16_t aim_jni_proxy_call_nonvirtual_short_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  return (int16_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           &arguments, 'S'));
}

extern int32_t aim_jni_proxy_call_nonvirtual_int_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
int32_t aim_jni_proxy_call_nonvirtual_int_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  return (int32_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           &arguments, 'I'));
}

extern int64_t aim_jni_proxy_call_nonvirtual_long_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
int64_t aim_jni_proxy_call_nonvirtual_long_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  return (int64_t)(ProxyCallNonvirtualBits(raw_env, object, clazz, method,
                                           &arguments, 'J'));
}

extern float aim_jni_proxy_call_nonvirtual_float_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
float aim_jni_proxy_call_nonvirtual_float_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  const uint32_t bits = (uint32_t)(ProxyCallNonvirtualBits(
      raw_env, object, clazz, method, &arguments, 'F'));
  float value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

extern double aim_jni_proxy_call_nonvirtual_double_method(
    void* raw_env, void* object, void* clazz, void* method, ...);
double aim_jni_proxy_call_nonvirtual_double_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  const uint64_t bits =
      ProxyCallNonvirtualBits(raw_env, object, clazz, method, &arguments, 'D');
  double value = 0;
  memcpy(&value, &bits, sizeof(value));
  return value;
}

extern void aim_jni_proxy_call_nonvirtual_void_method(void* raw_env,
                                                             void* object,
                                                             void* clazz,
                                                             void* method, ...);
void aim_jni_proxy_call_nonvirtual_void_method_captured(
    void* raw_env, void* object, void* clazz, void* method,
    uint8_t* gp_registers, uint8_t* fp_registers, uint8_t* caller_stack) {
  AndroidArm64VaList arguments =
      CapturedVaList(gp_registers, fp_registers, caller_stack, 4);
  (void)(ProxyCallNonvirtualBits(raw_env, object, clazz, method, &arguments,
                                 'V'));
}

static const RawJniSlot kNativeTable[AIM_JNI_NATIVE_SLOT_COUNT] = {
    [AIM_JNI_SLOT_GetVersion] = (RawJniSlot)ProxyGetVersion,
    [AIM_JNI_SLOT_FindClass] = (RawJniSlot)ProxyFindClass,
    [AIM_JNI_SLOT_FromReflectedMethod] =
        (RawJniSlot)ProxyFromReflectedMethod,
    [AIM_JNI_SLOT_FromReflectedField] = (RawJniSlot)ProxyFromReflectedField,
    [AIM_JNI_SLOT_ToReflectedMethod] = (RawJniSlot)ProxyToReflectedMethod,
    [AIM_JNI_SLOT_ToReflectedField] = (RawJniSlot)ProxyToReflectedField,
    [AIM_JNI_SLOT_GetSuperclass] = (RawJniSlot)ProxyGetSuperclass,
    [AIM_JNI_SLOT_IsAssignableFrom] =
        (RawJniSlot)ProxyIsAssignableFrom,
    [AIM_JNI_SLOT_Throw] = (RawJniSlot)ProxyThrow,
    [AIM_JNI_SLOT_ThrowNew] = (RawJniSlot)ProxyThrowNew,
    [AIM_JNI_SLOT_ExceptionOccurred] =
        (RawJniSlot)ProxyExceptionOccurred,
    [AIM_JNI_SLOT_ExceptionDescribe] =
        (RawJniSlot)ProxyExceptionDescribe,
    [AIM_JNI_SLOT_ExceptionClear] = (RawJniSlot)ProxyExceptionClear,
    [AIM_JNI_SLOT_FatalError] = (RawJniSlot)ProxyFatalError,
    [AIM_JNI_SLOT_PushLocalFrame] = (RawJniSlot)ProxyPushLocalFrame,
    [AIM_JNI_SLOT_PopLocalFrame] = (RawJniSlot)ProxyPopLocalFrame,
    [AIM_JNI_SLOT_NewGlobalRef] = (RawJniSlot)ProxyNewGlobalRef,
    [AIM_JNI_SLOT_DeleteGlobalRef] = (RawJniSlot)ProxyDeleteGlobalRef,
    [AIM_JNI_SLOT_DeleteLocalRef] = (RawJniSlot)ProxyDeleteLocalRef,
    [AIM_JNI_SLOT_IsSameObject] = (RawJniSlot)ProxyIsSameObject,
    [AIM_JNI_SLOT_NewDirectByteBuffer] =
        (RawJniSlot)ProxyNewDirectByteBuffer,
    [AIM_JNI_SLOT_GetDirectBufferAddress] =
        (RawJniSlot)ProxyGetDirectBufferAddress,
    [AIM_JNI_SLOT_GetDirectBufferCapacity] =
        (RawJniSlot)ProxyGetDirectBufferCapacity,
    [AIM_JNI_SLOT_NewLocalRef] = (RawJniSlot)ProxyNewLocalRef,
    [AIM_JNI_SLOT_GetObjectClass] = (RawJniSlot)ProxyGetObjectClass,
    [AIM_JNI_SLOT_IsInstanceOf] = (RawJniSlot)ProxyIsInstanceOf,
    [AIM_JNI_SLOT_GetMethodID] = (RawJniSlot)ProxyGetMethodID,
    [AIM_JNI_SLOT_NewObject] =
        (RawJniSlot)aim_jni_proxy_new_object,
    [AIM_JNI_SLOT_NewObjectV] = (RawJniSlot)ProxyNewObjectV,
    [AIM_JNI_SLOT_NewObjectA] = (RawJniSlot)ProxyNewObjectA,
    [AIM_JNI_SLOT_CallVoidMethod] =
        (RawJniSlot)aim_jni_proxy_call_void_method,
    [AIM_JNI_SLOT_CallObjectMethod] =
        (RawJniSlot)aim_jni_proxy_call_object_method,
    [AIM_JNI_SLOT_CallIntMethod] =
        (RawJniSlot)aim_jni_proxy_call_int_method,
    [AIM_JNI_SLOT_CallObjectMethodV] =
        (RawJniSlot)ProxyCallObjectMethodV,
    [AIM_JNI_SLOT_CallBooleanMethodV] =
        (RawJniSlot)ProxyCallBooleanMethodV,
    [AIM_JNI_SLOT_CallByteMethodV] = (RawJniSlot)ProxyCallByteMethodV,
    [AIM_JNI_SLOT_CallCharMethodV] = (RawJniSlot)ProxyCallCharMethodV,
    [AIM_JNI_SLOT_CallShortMethodV] = (RawJniSlot)ProxyCallShortMethodV,
    [AIM_JNI_SLOT_CallIntMethodV] = (RawJniSlot)ProxyCallIntMethodV,
    [AIM_JNI_SLOT_CallLongMethodV] =
        (RawJniSlot)ProxyCallLongMethodV,
    [AIM_JNI_SLOT_CallFloatMethodV] = (RawJniSlot)ProxyCallFloatMethodV,
    [AIM_JNI_SLOT_CallDoubleMethodV] =
        (RawJniSlot)ProxyCallDoubleMethodV,
    [AIM_JNI_SLOT_CallVoidMethodV] =
        (RawJniSlot)ProxyCallVoidMethodV,
#define INSTALL_CALL_METHOD_A(Name)                                          \
  [AIM_JNI_SLOT_Call##Name##MethodA] =                                \
      (RawJniSlot)ProxyCall##Name##MethodA,
    INSTALL_CALL_METHOD_A(Object)
    INSTALL_CALL_METHOD_A(Boolean)
    INSTALL_CALL_METHOD_A(Byte)
    INSTALL_CALL_METHOD_A(Char)
    INSTALL_CALL_METHOD_A(Short)
    INSTALL_CALL_METHOD_A(Int)
    INSTALL_CALL_METHOD_A(Long)
    INSTALL_CALL_METHOD_A(Float)
    INSTALL_CALL_METHOD_A(Double)
    INSTALL_CALL_METHOD_A(Void)
#undef INSTALL_CALL_METHOD_A
    [AIM_JNI_SLOT_GetFieldID] = (RawJniSlot)ProxyGetFieldID,
#define INSTALL_FIELD_PROXY(Name)                                             \
  [AIM_JNI_SLOT_Get##Name##Field] = (RawJniSlot)ProxyGet##Name##Field, \
  [AIM_JNI_SLOT_Set##Name##Field] = (RawJniSlot)ProxySet##Name##Field,
    INSTALL_FIELD_PROXY(Object)
    INSTALL_FIELD_PROXY(Boolean)
    INSTALL_FIELD_PROXY(Byte)
    INSTALL_FIELD_PROXY(Char)
    INSTALL_FIELD_PROXY(Short)
    INSTALL_FIELD_PROXY(Int)
    INSTALL_FIELD_PROXY(Long)
    INSTALL_FIELD_PROXY(Float)
    INSTALL_FIELD_PROXY(Double)
#undef INSTALL_FIELD_PROXY
    [AIM_JNI_SLOT_GetStaticMethodID] =
        (RawJniSlot)ProxyGetStaticMethodID,
    [AIM_JNI_SLOT_CallStaticVoidMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_void_method,
    [AIM_JNI_SLOT_CallStaticObjectMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_object_method,
    [AIM_JNI_SLOT_CallStaticIntMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_int_method,
    [AIM_JNI_SLOT_CallStaticObjectMethodV] =
        (RawJniSlot)ProxyCallStaticObjectMethodV,
    [AIM_JNI_SLOT_CallStaticBooleanMethodV] =
        (RawJniSlot)ProxyCallStaticBooleanMethodV,
    [AIM_JNI_SLOT_CallStaticByteMethodV] =
        (RawJniSlot)ProxyCallStaticByteMethodV,
    [AIM_JNI_SLOT_CallStaticCharMethodV] =
        (RawJniSlot)ProxyCallStaticCharMethodV,
    [AIM_JNI_SLOT_CallStaticShortMethodV] =
        (RawJniSlot)ProxyCallStaticShortMethodV,
    [AIM_JNI_SLOT_CallStaticIntMethodV] =
        (RawJniSlot)ProxyCallStaticIntMethodV,
    [AIM_JNI_SLOT_CallStaticLongMethodV] =
        (RawJniSlot)ProxyCallStaticLongMethodV,
    [AIM_JNI_SLOT_CallStaticFloatMethodV] =
        (RawJniSlot)ProxyCallStaticFloatMethodV,
    [AIM_JNI_SLOT_CallStaticDoubleMethodV] =
        (RawJniSlot)ProxyCallStaticDoubleMethodV,
    [AIM_JNI_SLOT_CallStaticVoidMethodV] =
        (RawJniSlot)ProxyCallStaticVoidMethodV,
#define INSTALL_CALL_STATIC_METHOD_A(Name)                                   \
  [AIM_JNI_SLOT_CallStatic##Name##MethodA] =                          \
      (RawJniSlot)ProxyCallStatic##Name##MethodA,
    INSTALL_CALL_STATIC_METHOD_A(Object)
    INSTALL_CALL_STATIC_METHOD_A(Boolean)
    INSTALL_CALL_STATIC_METHOD_A(Byte)
    INSTALL_CALL_STATIC_METHOD_A(Char)
    INSTALL_CALL_STATIC_METHOD_A(Short)
    INSTALL_CALL_STATIC_METHOD_A(Int)
    INSTALL_CALL_STATIC_METHOD_A(Long)
    INSTALL_CALL_STATIC_METHOD_A(Float)
    INSTALL_CALL_STATIC_METHOD_A(Double)
    INSTALL_CALL_STATIC_METHOD_A(Void)
#undef INSTALL_CALL_STATIC_METHOD_A
    [AIM_JNI_SLOT_GetStaticFieldID] =
        (RawJniSlot)ProxyGetStaticFieldID,
#define INSTALL_STATIC_FIELD_PROXY(Name)                                      \
  [AIM_JNI_SLOT_GetStatic##Name##Field] =                              \
      (RawJniSlot)ProxyGetStatic##Name##Field,                                \
  [AIM_JNI_SLOT_SetStatic##Name##Field] =                              \
      (RawJniSlot)ProxySetStatic##Name##Field,
    INSTALL_STATIC_FIELD_PROXY(Object)
    INSTALL_STATIC_FIELD_PROXY(Boolean)
    INSTALL_STATIC_FIELD_PROXY(Byte)
    INSTALL_STATIC_FIELD_PROXY(Char)
    INSTALL_STATIC_FIELD_PROXY(Short)
    INSTALL_STATIC_FIELD_PROXY(Int)
    INSTALL_STATIC_FIELD_PROXY(Long)
    INSTALL_STATIC_FIELD_PROXY(Float)
    INSTALL_STATIC_FIELD_PROXY(Double)
#undef INSTALL_STATIC_FIELD_PROXY
    [AIM_JNI_SLOT_NewString] = (RawJniSlot)ProxyNewString,
    [AIM_JNI_SLOT_GetStringLength] = (RawJniSlot)ProxyGetStringLength,
    [AIM_JNI_SLOT_GetStringChars] = (RawJniSlot)ProxyGetStringChars,
    [AIM_JNI_SLOT_ReleaseStringChars] =
        (RawJniSlot)ProxyReleaseStringChars,
    [AIM_JNI_SLOT_NewStringUTF] = (RawJniSlot)ProxyNewStringUTF,
    [AIM_JNI_SLOT_GetStringUTFLength] =
        (RawJniSlot)ProxyGetStringUTFLength,
    [AIM_JNI_SLOT_GetStringUTFChars] =
        (RawJniSlot)ProxyGetStringUTFChars,
    [AIM_JNI_SLOT_ReleaseStringUTFChars] =
        (RawJniSlot)ProxyReleaseStringUTFChars,
    [AIM_JNI_SLOT_GetArrayLength] = (RawJniSlot)ProxyGetArrayLength,
    [AIM_JNI_SLOT_NewObjectArray] = (RawJniSlot)ProxyNewObjectArray,
    [AIM_JNI_SLOT_GetObjectArrayElement] =
        (RawJniSlot)ProxyGetObjectArrayElement,
    [AIM_JNI_SLOT_SetObjectArrayElement] =
        (RawJniSlot)ProxySetObjectArrayElement,
#define INSTALL_PRIMITIVE_ARRAY_PROXY(Name)                                    \
  [AIM_JNI_SLOT_New##Name##Array] =                                    \
      (RawJniSlot)ProxyNew##Name##Array,                                      \
  [AIM_JNI_SLOT_Get##Name##ArrayElements] =                            \
      (RawJniSlot)ProxyGet##Name##ArrayElements,                              \
  [AIM_JNI_SLOT_Release##Name##ArrayElements] =                        \
      (RawJniSlot)ProxyRelease##Name##ArrayElements,                          \
  [AIM_JNI_SLOT_Get##Name##ArrayRegion] =                              \
      (RawJniSlot)ProxyGet##Name##ArrayRegion,                                \
  [AIM_JNI_SLOT_Set##Name##ArrayRegion] =                              \
      (RawJniSlot)ProxySet##Name##ArrayRegion,
    INSTALL_PRIMITIVE_ARRAY_PROXY(Boolean)
    INSTALL_PRIMITIVE_ARRAY_PROXY(Char)
    INSTALL_PRIMITIVE_ARRAY_PROXY(Short)
    INSTALL_PRIMITIVE_ARRAY_PROXY(Int)
    INSTALL_PRIMITIVE_ARRAY_PROXY(Long)
    INSTALL_PRIMITIVE_ARRAY_PROXY(Float)
    INSTALL_PRIMITIVE_ARRAY_PROXY(Double)
#undef INSTALL_PRIMITIVE_ARRAY_PROXY
    [AIM_JNI_SLOT_NewByteArray] = (RawJniSlot)ProxyNewByteArray,
    [AIM_JNI_SLOT_GetByteArrayElements] =
        (RawJniSlot)ProxyGetByteArrayElements,
    [AIM_JNI_SLOT_ReleaseByteArrayElements] =
        (RawJniSlot)ProxyReleaseByteArrayElements,
    [AIM_JNI_SLOT_GetByteArrayRegion] =
        (RawJniSlot)ProxyGetByteArrayRegion,
    [AIM_JNI_SLOT_SetByteArrayRegion] =
        (RawJniSlot)ProxySetByteArrayRegion,
    [AIM_JNI_SLOT_RegisterNatives] = (RawJniSlot)ProxyRegisterNatives,
    [AIM_JNI_SLOT_UnregisterNatives] =
        (RawJniSlot)ProxyUnregisterNatives,
    [AIM_JNI_SLOT_MonitorEnter] = (RawJniSlot)ProxyMonitorEnter,
    [AIM_JNI_SLOT_MonitorExit] = (RawJniSlot)ProxyMonitorExit,
    [AIM_JNI_SLOT_GetJavaVM] = (RawJniSlot)ProxyGetJavaVM,
    [AIM_JNI_SLOT_GetStringRegion] = (RawJniSlot)ProxyGetStringRegion,
    [AIM_JNI_SLOT_GetStringUTFRegion] =
        (RawJniSlot)ProxyGetStringUTFRegion,
    [AIM_JNI_SLOT_GetPrimitiveArrayCritical] =
        (RawJniSlot)ProxyGetPrimitiveArrayCritical,
    [AIM_JNI_SLOT_ReleasePrimitiveArrayCritical] =
        (RawJniSlot)ProxyReleasePrimitiveArrayCritical,
    [AIM_JNI_SLOT_GetStringCritical] =
        (RawJniSlot)ProxyGetStringCritical,
    [AIM_JNI_SLOT_ReleaseStringCritical] =
        (RawJniSlot)ProxyReleaseStringCritical,
    [AIM_JNI_SLOT_NewWeakGlobalRef] =
        (RawJniSlot)ProxyNewWeakGlobalRef,
    [AIM_JNI_SLOT_DeleteWeakGlobalRef] =
        (RawJniSlot)ProxyDeleteWeakGlobalRef,
    [AIM_JNI_SLOT_ExceptionCheck] = (RawJniSlot)ProxyExceptionCheck,
    /* Remaining slots. */
    [AIM_JNI_SLOT_DefineClass] = (RawJniSlot)ProxyDefineClass,
    [AIM_JNI_SLOT_EnsureLocalCapacity] =
        (RawJniSlot)ProxyEnsureLocalCapacity,
    [AIM_JNI_SLOT_AllocObject] = (RawJniSlot)ProxyAllocObject,
    [AIM_JNI_SLOT_GetObjectRefType] = (RawJniSlot)ProxyGetObjectRefType,
    [AIM_JNI_SLOT_CallNonvirtualObjectMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_object_method,
    [AIM_JNI_SLOT_CallNonvirtualObjectMethodV] =
        (RawJniSlot)ProxyCallNonvirtualObjectMethodV,
    [AIM_JNI_SLOT_CallNonvirtualObjectMethodA] =
        (RawJniSlot)ProxyCallNonvirtualObjectMethodA,
    [AIM_JNI_SLOT_CallBooleanMethod] =
        (RawJniSlot)aim_jni_proxy_call_boolean_method,
    [AIM_JNI_SLOT_CallStaticBooleanMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_boolean_method,
    [AIM_JNI_SLOT_CallNonvirtualBooleanMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_boolean_method,
    [AIM_JNI_SLOT_CallNonvirtualBooleanMethodV] =
        (RawJniSlot)ProxyCallNonvirtualBooleanMethodV,
    [AIM_JNI_SLOT_CallNonvirtualBooleanMethodA] =
        (RawJniSlot)ProxyCallNonvirtualBooleanMethodA,
    [AIM_JNI_SLOT_CallByteMethod] =
        (RawJniSlot)aim_jni_proxy_call_byte_method,
    [AIM_JNI_SLOT_CallStaticByteMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_byte_method,
    [AIM_JNI_SLOT_CallNonvirtualByteMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_byte_method,
    [AIM_JNI_SLOT_CallNonvirtualByteMethodV] =
        (RawJniSlot)ProxyCallNonvirtualByteMethodV,
    [AIM_JNI_SLOT_CallNonvirtualByteMethodA] =
        (RawJniSlot)ProxyCallNonvirtualByteMethodA,
    [AIM_JNI_SLOT_CallCharMethod] =
        (RawJniSlot)aim_jni_proxy_call_char_method,
    [AIM_JNI_SLOT_CallStaticCharMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_char_method,
    [AIM_JNI_SLOT_CallNonvirtualCharMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_char_method,
    [AIM_JNI_SLOT_CallNonvirtualCharMethodV] =
        (RawJniSlot)ProxyCallNonvirtualCharMethodV,
    [AIM_JNI_SLOT_CallNonvirtualCharMethodA] =
        (RawJniSlot)ProxyCallNonvirtualCharMethodA,
    [AIM_JNI_SLOT_CallShortMethod] =
        (RawJniSlot)aim_jni_proxy_call_short_method,
    [AIM_JNI_SLOT_CallStaticShortMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_short_method,
    [AIM_JNI_SLOT_CallNonvirtualShortMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_short_method,
    [AIM_JNI_SLOT_CallNonvirtualShortMethodV] =
        (RawJniSlot)ProxyCallNonvirtualShortMethodV,
    [AIM_JNI_SLOT_CallNonvirtualShortMethodA] =
        (RawJniSlot)ProxyCallNonvirtualShortMethodA,
    [AIM_JNI_SLOT_CallNonvirtualIntMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_int_method,
    [AIM_JNI_SLOT_CallNonvirtualIntMethodV] =
        (RawJniSlot)ProxyCallNonvirtualIntMethodV,
    [AIM_JNI_SLOT_CallNonvirtualIntMethodA] =
        (RawJniSlot)ProxyCallNonvirtualIntMethodA,
    [AIM_JNI_SLOT_CallLongMethod] =
        (RawJniSlot)aim_jni_proxy_call_long_method,
    [AIM_JNI_SLOT_CallStaticLongMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_long_method,
    [AIM_JNI_SLOT_CallNonvirtualLongMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_long_method,
    [AIM_JNI_SLOT_CallNonvirtualLongMethodV] =
        (RawJniSlot)ProxyCallNonvirtualLongMethodV,
    [AIM_JNI_SLOT_CallNonvirtualLongMethodA] =
        (RawJniSlot)ProxyCallNonvirtualLongMethodA,
    [AIM_JNI_SLOT_CallFloatMethod] =
        (RawJniSlot)aim_jni_proxy_call_float_method,
    [AIM_JNI_SLOT_CallStaticFloatMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_float_method,
    [AIM_JNI_SLOT_CallNonvirtualFloatMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_float_method,
    [AIM_JNI_SLOT_CallNonvirtualFloatMethodV] =
        (RawJniSlot)ProxyCallNonvirtualFloatMethodV,
    [AIM_JNI_SLOT_CallNonvirtualFloatMethodA] =
        (RawJniSlot)ProxyCallNonvirtualFloatMethodA,
    [AIM_JNI_SLOT_CallDoubleMethod] =
        (RawJniSlot)aim_jni_proxy_call_double_method,
    [AIM_JNI_SLOT_CallStaticDoubleMethod] =
        (RawJniSlot)aim_jni_proxy_call_static_double_method,
    [AIM_JNI_SLOT_CallNonvirtualDoubleMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_double_method,
    [AIM_JNI_SLOT_CallNonvirtualDoubleMethodV] =
        (RawJniSlot)ProxyCallNonvirtualDoubleMethodV,
    [AIM_JNI_SLOT_CallNonvirtualDoubleMethodA] =
        (RawJniSlot)ProxyCallNonvirtualDoubleMethodA,
    [AIM_JNI_SLOT_CallNonvirtualVoidMethod] =
        (RawJniSlot)aim_jni_proxy_call_nonvirtual_void_method,
    [AIM_JNI_SLOT_CallNonvirtualVoidMethodV] =
        (RawJniSlot)ProxyCallNonvirtualVoidMethodV,
    [AIM_JNI_SLOT_CallNonvirtualVoidMethodA] =
        (RawJniSlot)ProxyCallNonvirtualVoidMethodA,
};

static const RawJniSlot kInvokeTable[AIM_JNI_INVOKE_SLOT_COUNT] = {
    [AIM_JNI_INVOKE_SLOT_AttachCurrentThread] =
        (RawJniSlot)ProxyAttachCurrentThread,
    [AIM_JNI_INVOKE_SLOT_DetachCurrentThread] =
        (RawJniSlot)ProxyDetachCurrentThread,
    [AIM_JNI_INVOKE_SLOT_GetEnv] = (RawJniSlot)ProxyGetEnv,
    [AIM_JNI_INVOKE_SLOT_AttachCurrentThreadAsDaemon] =
        (RawJniSlot)ProxyAttachCurrentThreadAsDaemon,
};

AimJniProxy*
aim_jni_proxy_init(void* storage, size_t storage_size,
                          const AimJniBackend* backend) {
  if (storage == NULL || storage_size < sizeof(struct AimJniProxy) ||
      (uintptr_t)storage % _Alignof(struct AimJniProxy) != 0 ||
      backend == NULL || backend->find_class == NULL ||
      backend->register_natives == NULL || backend->throw_new == NULL)
    return NULL;
  struct AimJniProxy* proxy = (struct AimJniProxy*)storage;
  memset(proxy, 0, sizeof(*proxy));
  proxy->backend = *backend;
  proxy->env.functions = kNativeTable;
  proxy->env.owner = proxy;
  proxy->vm.functions = kInvokeTable;
  proxy->vm.owner = proxy;
  proxy->magic = kProxyMagic;
  return proxy;
}

void* aim_jni_proxy_java_vm(AimJniProxy* proxy) {
  if (proxy == NULL || proxy->magic != kProxyMagic) return NULL;
  return &proxy->vm;
}
