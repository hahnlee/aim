#ifndef AIM_BIONIC_DSO_LIFECYCLE_H_
#define AIM_BIONIC_DSO_LIFECYCLE_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*AimBionicDsoDestructor)(void*);
typedef void (*AimBionicDsoFunction)(void);
typedef int (*AimBionicDladdrCallback)(const void*, void*);

int aim_bionic___cxa_atexit(AimBionicDsoDestructor function,
                                   void* argument, void* dso);
int aim_bionic___cxa_thread_atexit_impl(
    AimBionicDsoDestructor function, void* argument, void* dso);
void aim_bionic___cxa_finalize(void* dso);
AimBionicDsoFunction aim_bionic_dso_lifecycle_resolve(
    const char* name);
void aim_bionic_dso_install_dladdr(
    AimBionicDladdrCallback callback);

int aim_bionic_dso_cxa_atexit_core(
    AimBionicDsoDestructor function, void* argument, void* dso);
void aim_bionic_dso_cxa_finalize_core(void* dso);
int aim_bionic_dso_cxa_thread_atexit_core(
    AimBionicDsoDestructor function, void* argument, void* dso);

typedef struct AimBionicDsoLifecycleOwner
    AimBionicDsoLifecycleOwner;

AimBionicDsoLifecycleOwner*
aim_bionic_dso_lifecycle_owner_create(void);
void aim_bionic_dso_lifecycle_owner_destroy(
    AimBionicDsoLifecycleOwner* owner);
int aim_bionic_dso_lifecycle_publish_image(void* owner,
                                                  uintptr_t start,
                                                  uintptr_t end);
int aim_bionic_dso_lifecycle_finalize_image(void* owner,
                                                   uintptr_t start,
                                                   uintptr_t end);

#ifdef __cplusplus
}  // extern "C"
#endif

#endif
