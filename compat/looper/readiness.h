#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct AimLooperQueue AimLooperQueue;
typedef struct AimLooperReadyEvent {
    uint64_t token;
    uint32_t flags; // READ=1, WRITE=2, EOF=4, ERROR=8; not Linux epoll constants.
    uint32_t reserved;
} AimLooperReadyEvent;
// Return zero or host errno, without setting errno TLS. Destroy after quiescence.
int aim_looper_queue_create(AimLooperQueue** out);
void aim_looper_queue_destroy(AimLooperQueue* queue);
typedef struct AimLooperRegistration {
    uint64_t token;
    uint32_t mask; // READ=1, WRITE=2, ERROR_ONLY=4 (exclusive); zero removes all.
    uint32_t reserved; // Must be zero.
} AimLooperRegistration;
typedef struct AimLooperTransitionResult {
    int32_t operation_error;
    int32_t rollback_error; // Nonzero: caller must discard/rebuild kernel set.
} AimLooperTransitionResult;
// Caller owns previous state and serializes changes/close on this host FD number.
// Already-closed descriptors are allowed and return kernel errors.
// Success is both errors zero. Caller must not publish next state on failure;
// rollback_error distinguishes restored old state from an inconsistent kernel set.
AimLooperTransitionResult aim_looper_queue_transition(
    const AimLooperQueue*, int fd,
    AimLooperRegistration previous, AimLooperRegistration next);
// Borrowed host FD number (may be closed); one filter per call (read=1 or write=2).
// operation=0 replaces/registers that filter, 1 deletes, 2 updates only an
// existing kernel filter (ENOENT after close/recycle is preserved).
int aim_looper_queue_change(const AimLooperQueue*, int fd,
                                 uint32_t interest, uint32_t operation, uint64_t token);
// Read/write results within one kernel batch merge by token, preserving flags
// and first-occurrence order. Caller assigns unique registration sequence tokens.
// capacity 1..1024; timeout_ms=-1 or >=0; valid kernel errors set count=0.
int aim_looper_queue_wait(const AimLooperQueue*, AimLooperReadyEvent*,
                               uint32_t capacity, int timeout_ms, uint32_t* count);
#ifdef __cplusplus
}
#endif
