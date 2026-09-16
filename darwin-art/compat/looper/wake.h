#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif
// Opaque Rust owner. Callers synchronize destruction against every operation.
typedef struct DarwinArtLooperWake DarwinArtLooperWake;
// Zero or host errno; these functions do not promise to set errno TLS.
int darwin_art_looper_wake_create(DarwinArtLooperWake** out);
void darwin_art_looper_wake_destroy(DarwinArtLooperWake* wake);
// Borrowed host descriptor: poll only, never read or close directly.
int darwin_art_looper_wake_fd(const DarwinArtLooperWake* wake);
int darwin_art_looper_wake_signal(const DarwinArtLooperWake* wake, uint64_t increment);
int darwin_art_looper_wake_drain(const DarwinArtLooperWake* wake, uint64_t* out);
#ifdef __cplusplus
}
#endif
