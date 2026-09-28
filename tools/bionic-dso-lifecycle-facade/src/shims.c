#include "aim_bionic_dso_lifecycle.h"

#include <errno.h>
#include <stddef.h>
#include <stdatomic.h>

extern void* aim_bionic_android_dlopen_ext(const char*, int,
                                                   const void*);
extern int aim_bionic_dlclose(void*);
extern char* aim_bionic_dlerror(void);
extern void* aim_bionic_dlopen(const char*, int);
extern void* aim_bionic_dlsym(void*, const char*);

int aim_bionic___cxa_atexit(AimBionicDsoDestructor function,
                                   void* argument, void* dso) {
  const int saved_host_errno = errno;
  const int result =
      aim_bionic_dso_cxa_atexit_core(function, argument, dso);
  errno = saved_host_errno;
  return result;
}

int aim_bionic___cxa_thread_atexit_impl(
    AimBionicDsoDestructor function, void* argument, void* dso) {
  const int saved_host_errno = errno;
  const int result =
      aim_bionic_dso_cxa_thread_atexit_core(function, argument, dso);
  errno = saved_host_errno;
  return result;
}

void aim_bionic___cxa_finalize(void* dso) {
  const int saved_host_errno = errno;
  aim_bionic_dso_cxa_finalize_core(dso);
  errno = saved_host_errno;
}

int aim_bionic___register_atfork(void* prepare, void* parent,
                                        void* child, void* arg) {
  (void)prepare;
  (void)parent;
  (void)child;
  (void)arg;
  return 0;
}

typedef int (*AimDladdrCallback)(const void*, void*);
static _Atomic(AimDladdrCallback) g_dladdr_callback;

void aim_bionic_dso_install_dladdr(AimDladdrCallback callback) {
  atomic_store_explicit(&g_dladdr_callback, callback, memory_order_release);
}

int aim_bionic_dladdr(const void* address, void* info) {
  AimDladdrCallback callback =
      atomic_load_explicit(&g_dladdr_callback, memory_order_acquire);
  return callback == NULL ? 0 : callback(address, info);
}

static int NameCompare(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return (unsigned char)*left < (unsigned char)*right
             ? -1
             : ((unsigned char)*left != (unsigned char)*right);
}

typedef struct Binding {
  const char* name;
  AimBionicDsoFunction address;
} Binding;

static const Binding kBindings[] = {
    {"__cxa_atexit",
     (AimBionicDsoFunction)aim_bionic___cxa_atexit},
    {"__cxa_finalize",
     (AimBionicDsoFunction)aim_bionic___cxa_finalize},
    {"__cxa_thread_atexit_impl",
     (AimBionicDsoFunction)aim_bionic___cxa_thread_atexit_impl},
    {"__register_atfork",
     (AimBionicDsoFunction)aim_bionic___register_atfork},
    {"android_dlopen_ext",
     (AimBionicDsoFunction)aim_bionic_android_dlopen_ext},
    {"dladdr", (AimBionicDsoFunction)aim_bionic_dladdr},
    {"dlclose", (AimBionicDsoFunction)aim_bionic_dlclose},
    {"dlerror", (AimBionicDsoFunction)aim_bionic_dlerror},
    {"dlopen", (AimBionicDsoFunction)aim_bionic_dlopen},
    {"dlsym", (AimBionicDsoFunction)aim_bionic_dlsym},
};

AimBionicDsoFunction aim_bionic_dso_lifecycle_resolve(
    const char* name) {
  if (name == NULL) return NULL;
  size_t low = 0;
  size_t high = sizeof(kBindings) / sizeof(kBindings[0]);
  while (low < high) {
    const size_t middle = low + (high - low) / 2;
    const int order = NameCompare(name, kBindings[middle].name);
    if (order == 0) return kBindings[middle].address;
    if (order < 0)
      high = middle;
    else
      low = middle + 1;
  }
  return NULL;
}
