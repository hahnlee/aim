#ifndef AIM_COMPAT_ICU_JNI_BYTESWAP_H_
#define AIM_COMPAT_ICU_JNI_BYTESWAP_H_

// GNU libc exposes these spellings from <byteswap.h>. Darwin's compiler has
// the same operations as builtins but the compatibility header is absent.
#define bswap_16(value) __builtin_bswap16(value)
#define bswap_32(value) __builtin_bswap32(value)
#define bswap_64(value) __builtin_bswap64(value)

#endif  // AIM_COMPAT_ICU_JNI_BYTESWAP_H_
