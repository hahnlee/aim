#pragma once
#include <stddef.h>

// Android 16's OpenBSD compatibility header uses this C23 function. NDK r28
// public headers predate the declaration. Implementation must be supplied by
// the guest binding, never an implicit declaration or ordinary memset alias.
#ifdef __cplusplus
extern "C" {
#endif
void* darwin_art_ftw_import_memset_explicit(void*, int, size_t);
#ifdef __cplusplus
}
#endif
