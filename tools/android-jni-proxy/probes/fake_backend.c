#include "aim_jni_proxy.h"

#include <stddef.h>
#include <stdint.h>
#include <string.h>

typedef struct FakeState {
  int find_bridge;
  int find_exception;
  int registered;
  int thrown;
  int calls;
  int nonvirtual_calls;
} FakeState;

/* Android AAPCS64 va_list, as the proxy hands it to the backend. */
typedef struct FakeAndroidVaList {
  uint8_t* stack;
  uint8_t* gr_top;
  uint8_t* vr_top;
  int32_t gr_offs;
  int32_t vr_offs;
} FakeAndroidVaList;

static uint64_t NextGp(FakeAndroidVaList* list) {
  uint64_t value = 0;
  if (list->gr_offs < 0) {
    memcpy(&value, list->gr_top + list->gr_offs, sizeof(value));
    list->gr_offs += 8;
  } else {
    memcpy(&value, list->stack, sizeof(value));
    list->stack += 8;
  }
  return value;
}

static double NextDouble(FakeAndroidVaList* list) {
  double value = 0;
  if (list->vr_offs < 0) {
    memcpy(&value, list->vr_top + list->vr_offs, sizeof(value));
    list->vr_offs += 16;
  } else {
    memcpy(&value, list->stack, sizeof(value));
    list->stack += 8;
  }
  return value;
}

/* Method tokens name the fixture's descriptors. */
static int kSumLong;    /* (IJ)J */
static int kMixDouble;  /* (DI)D */
static int kSumInt;     /* (II)I */
static int kHalfFloat;  /* (D)F */

static FakeState kState;
static int kBridgeClass;
static int kExceptionClass;
_Alignas(AIM_JNI_PROXY_STORAGE_ALIGNMENT)
static unsigned char kProxyStorage[AIM_JNI_PROXY_STORAGE_SIZE];
static AimJniProxy* kProxy;

static void* FakeFindClass(void* context, const char* name) {
  FakeState* state = (FakeState*)context;
  if (strcmp(name, "fixture/Bridge") == 0) {
    ++state->find_bridge;
    return &kBridgeClass;
  }
  if (strcmp(name, "java/lang/RuntimeException") == 0) {
    ++state->find_exception;
    return &kExceptionClass;
  }
  return NULL;
}

static int32_t FakeRegisterNatives(void* context, void* clazz,
                                   const AimJniNativeMethod* methods,
                                   int32_t count) {
  FakeState* state = (FakeState*)context;
  if (clazz != &kBridgeClass || count != 1 || methods == NULL ||
      strcmp(methods[0].name, "nativePing") != 0 ||
      strcmp(methods[0].signature, "()I") != 0 || methods[0].function == NULL)
    return AIM_JNI_ERR;
  ++state->registered;
  return AIM_JNI_OK;
}

static int32_t FakeThrowNew(void* context, void* clazz, const char* message) {
  FakeState* state = (FakeState*)context;
  if (clazz != &kExceptionClass || strcmp(message, "proxy-fixture") != 0)
    return AIM_JNI_ERR;
  ++state->thrown;
  return AIM_JNI_OK;
}

static void* FakeGetMethodId(void* context, void* clazz, const char* name,
                             const char* signature, int32_t is_static) {
  (void)context;
  (void)clazz;
  if (strcmp(name, "sumLong") == 0 && strcmp(signature, "(IJ)J") == 0 && !is_static)
    return &kSumLong;
  if (strcmp(name, "mixDouble") == 0 && strcmp(signature, "(DI)D") == 0 && is_static)
    return &kMixDouble;
  if (strcmp(name, "sumInt") == 0 && strcmp(signature, "(II)I") == 0 && !is_static)
    return &kSumInt;
  if (strcmp(name, "halfFloat") == 0 && strcmp(signature, "(D)F") == 0 && !is_static)
    return &kHalfFloat;
  return NULL;
}

static uint64_t FakeCall(void* method, FakeAndroidVaList* list, int32_t shorty) {
  if (method == &kSumLong && shorty == 'J') {
    const int32_t first = (int32_t)NextGp(list);
    const int64_t second = (int64_t)NextGp(list);
    return (uint64_t)(first + second);
  }
  if (method == &kMixDouble && shorty == 'D') {
    const double first = NextDouble(list);
    const int32_t second = (int32_t)NextGp(list);
    const double result = first + second;
    uint64_t bits = 0;
    memcpy(&bits, &result, sizeof(bits));
    return bits;
  }
  if (method == &kSumInt && shorty == 'I') {
    const int32_t first = (int32_t)NextGp(list);
    const int32_t second = (int32_t)NextGp(list);
    return (uint32_t)(first + second);
  }
  if (method == &kHalfFloat && shorty == 'F') {
    const float result = (float)(NextDouble(list) / 2);
    uint32_t bits = 0;
    memcpy(&bits, &result, sizeof(bits));
    return bits;
  }
  return 0;
}

static uint64_t FakeCallMethodV(void* context, void* object, void* method,
                                void* android_va_list, int32_t shorty,
                                int32_t is_static) {
  FakeState* state = (FakeState*)context;
  if (object == NULL || (is_static != 0) != (method == &kMixDouble)) return 0;
  ++state->calls;
  return FakeCall(method, (FakeAndroidVaList*)android_va_list, shorty);
}

static uint64_t FakeCallNonvirtualMethodV(void* context, void* object,
                                          void* clazz, void* method,
                                          void* android_va_list,
                                          int32_t shorty) {
  FakeState* state = (FakeState*)context;
  if (object == NULL || clazz != &kBridgeClass) return 0;
  ++state->nonvirtual_calls;
  return FakeCall(method, (FakeAndroidVaList*)android_va_list, shorty);
}

void aim_jni_fixture_reset(void) {
  memset(&kState, 0, sizeof(kState));
  const AimJniBackend backend = {
      .context = &kState,
      .find_class = FakeFindClass,
      .register_natives = FakeRegisterNatives,
      .throw_new = FakeThrowNew,
      .get_method_id = FakeGetMethodId,
      .call_method_v = FakeCallMethodV,
      .call_nonvirtual_method_v = FakeCallNonvirtualMethodV,
  };
  kProxy = aim_jni_proxy_init(kProxyStorage, sizeof(kProxyStorage), &backend);
}

void* aim_jni_fixture_vm(void) {
  return aim_jni_proxy_java_vm(kProxy);
}

int32_t aim_jni_fixture_passed(void) {
  return kProxy != NULL && kState.find_bridge == 1 && kState.find_exception == 1 &&
         kState.registered == 1 && kState.thrown == 1;
}

/* The Android ELF fixture's variadic, va_list and nonvirtual calls. */
int32_t aim_jni_fixture_calls_passed(void) {
  return kState.calls == 2 && kState.nonvirtual_calls == 2;
}
