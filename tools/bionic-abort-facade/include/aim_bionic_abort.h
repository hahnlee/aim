#ifndef AIM_BIONIC_ABORT_H_
#define AIM_BIONIC_ABORT_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct AimBionicAbortMessage {
  size_t size;
  char message[];
} AimBionicAbortMessage;

__attribute__((noreturn)) void aim_bionic_abort(void);
__attribute__((noreturn)) void aim_bionic___stack_chk_fail(void);
void aim_bionic_android_set_abort_message(const char* message);

void* aim_bionic_abort_resolve(const char* soname,
                                      const char* symbol,
                                      const char* version);
int aim_bionic_abort_capability(const char* capability);

/* Immutable process-lifetime inspection seam for conformance probes. */
const AimBionicAbortMessage*
aim_bionic_abort_message_for_test(void);

#ifdef __cplusplus
}
#endif

#endif  // AIM_BIONIC_ABORT_H_
