#ifndef AIM_ANDROID16_MEMORY_BYTESWAP_H_
#define AIM_ANDROID16_MEMORY_BYTESWAP_H_

// Android's Portability.h uses the glibc byteswap spellings. Darwin Clang
// provides identical operations as target-aware builtins.
#define bswap_16(value) __builtin_bswap16(value)
#define bswap_32(value) __builtin_bswap32(value)
#define bswap_64(value) __builtin_bswap64(value)

#endif  // AIM_ANDROID16_MEMORY_BYTESWAP_H_
