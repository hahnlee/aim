#ifndef DARWIN_ART_JNI_PROXY_H_
#define DARWIN_ART_JNI_PROXY_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
  DARWIN_ART_JNI_OK = 0,
  DARWIN_ART_JNI_ERR = -1,
  DARWIN_ART_JNI_EDETACHED = -2,
  DARWIN_ART_JNI_EVERSION = -3,
  DARWIN_ART_JNI_VERSION_1_1 = 0x00010001,
  DARWIN_ART_JNI_VERSION_1_2 = 0x00010002,
  DARWIN_ART_JNI_VERSION_1_4 = 0x00010004,
  DARWIN_ART_JNI_VERSION_1_6 = 0x00010006,
  DARWIN_ART_JNI_PROXY_STORAGE_SIZE = 256,
  DARWIN_ART_JNI_PROXY_STORAGE_ALIGNMENT = 16,
};

typedef struct DarwinArtJniNativeMethod {
  const char* name;
  const char* signature;
  void* function;
} DarwinArtJniNativeMethod;

typedef struct DarwinArtJniBackend {
  void* context;
  /* Returns the current host JNIEnv only to proxy wrappers. The guest never
   * receives this pointer or its function table. It may be null when the
   * forwarding subset is intentionally unavailable. */
  void* (*current_env)(void* context);
  int32_t (*attach_current_thread)(void* context, void* arguments,
                                   int32_t as_daemon);
  int32_t (*detach_current_thread)(void* context);
  void* (*find_class)(void* context, const char* name);
  /* methods[].function points to Android ELF code. The backend must retain its
   * Android ABI ownership and route later calls through a signature-audited
   * bridge; it must never register that address directly with ART. */
  int32_t (*register_natives)(void* context, void* clazz,
                              const DarwinArtJniNativeMethod* methods,
                              int32_t count);
  int32_t (*throw_new)(void* context, void* clazz, const char* message);
  /* Android arm64 and Darwin arm64 use different va_list layouts. These
   * callbacks retain the Java descriptor at method lookup time and translate
   * guest ...V calls to the host's jvalue[] (...A) ABI. */
  void* (*get_method_id)(void* context, void* clazz, const char* name,
                         const char* signature, int32_t is_static);
  uint64_t (*call_method_v)(void* context, void* object, void* method,
                            void* android_va_list, int32_t return_shorty,
                            int32_t is_static);
  /* Optional: CallNonvirtual<Type>MethodV of `method` as declared by `clazz`
   * on `object`, translated like call_method_v. Without it those slots fail
   * closed (return zero) instead of reaching the guest as NULL. */
  uint64_t (*call_nonvirtual_method_v)(void* context, void* object, void* clazz,
                                       void* method, void* android_va_list,
                                       int32_t return_shorty);
} DarwinArtJniBackend;

typedef struct DarwinArtJniProxy DarwinArtJniProxy;

/* The caller owns aligned storage and the backend context for the proxy's
 * entire lifetime. This module never receives or stores an ART function table.
 * Unsupported JNI slots are null: capability preflight must prove that a
 * library stays within the implemented subset before any ELF code executes. */
DarwinArtJniProxy* darwin_art_jni_proxy_init(void* storage, size_t storage_size,
                                             const DarwinArtJniBackend* backend);
void* darwin_art_jni_proxy_java_vm(DarwinArtJniProxy* proxy);

#ifdef __cplusplus
}
#endif

#endif
