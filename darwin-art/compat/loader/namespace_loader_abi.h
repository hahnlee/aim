#pragma once
#include <cstdint>
#include <cstddef>
#include "android_dlext_types.h"
struct android_namespace_t;
extern "C" void __loader_android_get_LD_LIBRARY_PATH(char*, size_t);
extern "C" void __loader_android_update_LD_LIBRARY_PATH(const char*);
extern "C" bool __loader_android_init_anonymous_namespace(const char*, const char*);
extern "C" void __loader_android_dlwarning(void*, void (*)(void*, const char*));
extern "C" void __loader_android_set_application_target_sdk_version(int);
extern "C" int __loader_android_get_application_target_sdk_version();
extern "C" void __loader_android_set_16kb_appcompat_mode(bool);
extern "C" int __loader_dlclose(void*);
extern "C" char* __loader_dlerror();
// Bionic libdl forwards the Android caller explicitly. Do not rediscover a
// return address inside these entry points: that would identify libdl instead.
extern "C" void* __loader_dlopen(const char*, int, const void*);
extern "C" void* __loader_dlsym(void*, const char*, const void*);
extern "C" void* __loader_dlvsym(void*, const char*, const char*, const void*);
extern "C" void* __loader_android_dlopen_ext(
    const char*, int, const android_dlextinfo*, const void*);
// Bionic libdl_android forwards its original caller in the seventh argument.
extern "C" android_namespace_t* __loader_android_create_namespace(
    const char*, const char*, const char*, uint64_t, const char*,
    android_namespace_t*, const void*);
extern "C" android_namespace_t* __loader_android_get_exported_namespace(const char*);
extern "C" bool __loader_android_link_namespaces(
    android_namespace_t*, android_namespace_t*, const char*);
