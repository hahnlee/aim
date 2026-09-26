#pragma once

#include <errno.h>
#include <stdint.h>

/*
 * Production SystemProperties JNI is a Darwin static object.  Keep its
 * property calls on the Android-owned providers instead of allowing the
 * non-Bionic android-base weak host map to satisfy them.
 */
struct prop_info;

extern "C" {
const prop_info* darwin_art_bionic___system_property_find(const char* name);
void darwin_art_bionic___system_property_read_callback(
    const prop_info* property,
    void (*callback)(void*, const char*, const char*, uint32_t), void* cookie);
int darwin_art_bionic_property_service_set(const char* key, const char* value);
int32_t darwin_art_bionic_errno_load(void);
void darwin_art_bionic_errno_store(int32_t android_errno);
/* Returns one and writes a Darwin errno only when the Android value has an
 * exact owner mapping.  Unknown Android values leave the output untouched. */
int darwin_art_bionic_errno_to_darwin(int32_t android_errno,
                                      int* darwin_errno);

/*
 * The AOSP JNI clears libc errno before calling the setter and inspects it on
 * failure.  The service client owns Android errno, so mirror that result into
 * the host errno cell only at this ABI boundary.  The setter's return value
 * and service failure remain unchanged; this is not a success fallback.
 */
inline int darwin_art_system_property_set_for_jni(const char* key,
                                                  const char* value) {
  errno = 0;
  darwin_art_bionic_errno_store(0);
  const int result = darwin_art_bionic_property_service_set(key, value);
  if (result != 0) {
    const int32_t android_errno = darwin_art_bionic_errno_load();
    int darwin_errno;
    if (android_errno != 0 &&
        darwin_art_bionic_errno_to_darwin(android_errno, &darwin_errno)) {
      errno = darwin_errno;
    } else {
      // Do not expose an incidental host errno from the service transport.
      // AOSP JNI uses errno==0 for its service-rejection diagnostic.
      errno = 0;
    }
  }
  return result;
}
}  // extern "C"

#define __system_property_find darwin_art_bionic___system_property_find
#define __system_property_read_callback \
  darwin_art_bionic___system_property_read_callback
#define __system_property_set darwin_art_system_property_set_for_jni
