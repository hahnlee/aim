#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct DarwinArtLooperQueue DarwinArtLooperQueue;
typedef struct DarwinArtLooperReadyEvent {
    uint64_t token;
    uint32_t flags; // READ=1, WRITE=2, EOF=4, ERROR=8; not Linux epoll constants.
    uint32_t reserved;
} DarwinArtLooperReadyEvent;
// Return zero or host errno, without setting errno TLS. Destroy after quiescence.
int darwin_art_looper_queue_create(DarwinArtLooperQueue** out);
void darwin_art_looper_queue_destroy(DarwinArtLooperQueue* queue);
typedef struct DarwinArtLooperRegistration {
    uint64_t token;
    uint32_t mask; // READ=1, WRITE=2, ERROR_ONLY=4 (exclusive); zero removes all.
    uint32_t reserved; // Must be zero.
} DarwinArtLooperRegistration;
typedef struct DarwinArtLooperTransitionResult {
    int32_t operation_error;
    int32_t rollback_error; // Nonzero: caller must discard/rebuild kernel set.
} DarwinArtLooperTransitionResult;
// Caller owns previous state and serializes changes/close on this host FD number.
// Already-closed descriptors are allowed and return kernel errors.
// Success is both errors zero. Caller must not publish next state on failure;
// rollback_error distinguishes restored old state from an inconsistent kernel set.
DarwinArtLooperTransitionResult darwin_art_looper_queue_transition(
    const DarwinArtLooperQueue*, int fd,
    DarwinArtLooperRegistration previous, DarwinArtLooperRegistration next);
// Borrowed host FD number (may be closed); one filter per call (read=1 or write=2).
// operation=0 replaces/registers that filter, 1 deletes, 2 updates only an
// existing kernel filter (ENOENT after close/recycle is preserved).
int darwin_art_looper_queue_change(const DarwinArtLooperQueue*, int fd,
                                 uint32_t interest, uint32_t operation, uint64_t token);
// Read/write results within one kernel batch merge by token, preserving flags
// and first-occurrence order. Caller assigns unique registration sequence tokens.
// capacity 1..1024; timeout_ms=-1 or >=0; valid kernel errors set count=0.
int darwin_art_looper_queue_wait(const DarwinArtLooperQueue*, DarwinArtLooperReadyEvent*,
                               uint32_t capacity, int timeout_ms, uint32_t* count);
#ifdef __cplusplus
}
#endif
