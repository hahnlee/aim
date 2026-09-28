#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif
// Opaque Rust owner. Callers synchronize destruction against every operation.
typedef struct AimLooperWake AimLooperWake;
// Zero or host errno; these functions do not promise to set errno TLS.
int aim_looper_wake_create(AimLooperWake** out);
void aim_looper_wake_destroy(AimLooperWake* wake);
// Borrowed host descriptor: poll only, never read or close directly.
int aim_looper_wake_fd(const AimLooperWake* wake);
int aim_looper_wake_signal(const AimLooperWake* wake, uint64_t increment);
int aim_looper_wake_drain(const AimLooperWake* wake, uint64_t* out);
#ifdef __cplusplus
}
#endif
