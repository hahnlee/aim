// Fake guest code shared by the P0 experiments. In the real system these
// bytes come from the Android ELF and the loader turns each `svc #0` into a
// branch to a per-site thunk with the LX_SVC body.
#include "lx.h"

    .text
    .p2align 4
// long guest_syscall6(long nr, long a0, ..., long a5): bionic's syscall(3).
    .globl _guest_syscall6
_guest_syscall6:
    mov     x8, x0
    mov     x0, x1
    mov     x1, x2
    mov     x2, x3
    mov     x3, x4
    mov     x4, x5
    mov     x5, x6
    LX_SVC
    ret
