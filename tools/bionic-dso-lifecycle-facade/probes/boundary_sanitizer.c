#include "aim_bionic_dso_lifecycle.h"

#include <errno.h>
#include <stdint.h>
#include <stdlib.h>

static AimBionicDsoDestructor gFunction;
static void* gArgument;
static void* gDso;
static unsigned gCalls;

int aim_bionic_dso_cxa_atexit_core(
    AimBionicDsoDestructor function, void* argument, void* dso) {
  if (function == NULL) return -1;
  gFunction = function;
  gArgument = argument;
  gDso = dso;
  return 0;
}

void aim_bionic_dso_cxa_finalize_core(void* dso) {
  if (gFunction == NULL || dso != gDso) return;
  AimBionicDsoDestructor function = gFunction;
  void* argument = gArgument;
  gFunction = NULL;
  function(argument);
}

int aim_bionic_dso_cxa_thread_atexit_core(
    AimBionicDsoDestructor function, void* argument, void* dso) {
  return aim_bionic_dso_cxa_atexit_core(function, argument, dso);
}

void* aim_bionic_android_dlopen_ext(const char* name, int flags,
                                            const void* info) {
  (void)name; (void)flags; (void)info; return NULL;
}
int aim_bionic_dlclose(void* handle) { (void)handle; return -1; }
char* aim_bionic_dlerror(void) { return NULL; }
void* aim_bionic_dlopen(const char* name, int flags) {
  (void)name; (void)flags; return NULL;
}
void* aim_bionic_dlsym(void* handle, const char* name) {
  (void)handle; (void)name; return NULL;
}

static void Destructor(void* argument) {
  if (argument != gArgument) abort();
  ++gCalls;
}

int main(void) {
  uintptr_t argument_storage = UINT64_C(0x1020304050607080);
  uintptr_t dso_storage = UINT64_C(0x8877665544332211);
  errno = 31001;
  if (aim_bionic___cxa_atexit(Destructor, &argument_storage,
                                     &dso_storage) != 0 ||
      errno != 31001 || gFunction != Destructor || gArgument != &argument_storage ||
      gDso != &dso_storage)
    return 1;
  if (aim_bionic_dso_lifecycle_resolve("__cxa_atexit") !=
          (AimBionicDsoFunction)aim_bionic___cxa_atexit ||
      aim_bionic_dso_lifecycle_resolve("__cxa_finalize") !=
          (AimBionicDsoFunction)aim_bionic___cxa_finalize ||
      aim_bionic_dso_lifecycle_resolve("__cxa_thread_atexit_impl") !=
          (AimBionicDsoFunction)aim_bionic___cxa_thread_atexit_impl ||
      aim_bionic_dso_lifecycle_resolve("cxa_finalize") != NULL)
    return 2;
  errno = 31002;
  aim_bionic___cxa_finalize(&dso_storage);
  if (errno != 31002 || gCalls != 1 || gFunction != NULL) return 3;
  aim_bionic___cxa_finalize(&dso_storage);
  if (gCalls != 1) return 4;
  return 0;
}
