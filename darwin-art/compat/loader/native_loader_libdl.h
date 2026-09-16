#pragma once
// This header is forced only for imported Android-owned loader compilation.
// Do not expose these flag replacements to host dlopen users.
#include <dlfcn.h>

// Android arm64 public libdl constants (bionic libc/include/dlfcn.h).
#undef RTLD_LAZY
#undef RTLD_NOW
#undef RTLD_LOCAL
#undef RTLD_GLOBAL
#undef RTLD_NOLOAD
#undef RTLD_NODELETE
#define RTLD_LAZY 1
#define RTLD_NOW 2
#define RTLD_LOCAL 0
#define RTLD_GLOBAL 0x100
#define RTLD_NOLOAD 4
#define RTLD_NODELETE 0x1000

extern "C" {
void* darwin_art_linker_dlopen(const char*, int);
int darwin_art_linker_dlclose(void*);
char* darwin_art_linker_dlerror();
void* darwin_art_linker_dlsym(void*, const char*);
}
// Android handles/errors must never be interpreted by macOS libdyld.
#define dlopen darwin_art_linker_dlopen
#define dlclose darwin_art_linker_dlclose
#define dlerror darwin_art_linker_dlerror
#define dlsym darwin_art_linker_dlsym
