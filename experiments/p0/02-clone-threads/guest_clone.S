// Fake guest code for the clone experiment.
#include "lx.h"

    .text
    .p2align 4
// bionic/libc/arch-arm64/bionic/__bionic_clone.S, with svc #0 redirected.
// int guest_bionic_clone(flags, child_stack, ptid, tls, ctid, fn, arg)
    .globl _guest_bionic_clone
_guest_bionic_clone:
    stp     x5, x6, [x1, #-16]!          // push fn, arg onto the child stack
    mov     x8, #220                     // __NR_clone
    LX_SVC
    cbz     x0, 1f
    ret                                  // parent (errno handling elided)
1:  mov     x29, #0                      // child: end of frame chain
    mov     x30, #0
    ldp     x0, x1, [sp], #16            // fn, arg
    b       _guest_start_thread

// Register-preservation probe: set callee-saved and SIMD registers, clone,
// and have the child record what it sees. Checks that the child starts with
// the parent's registers except x0/sp, exactly as Linux clone does.
// int guest_clone_regs_probe(flags, child_stack, ptid, tls, ctid, out)
    .globl _guest_clone_regs_probe
_guest_clone_regs_probe:
    stp     x19, x20, [sp, #-96]!
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     d8, d9, [sp, #80]
    mov     x19, #0x1919
    mov     x20, #0x2020
    mov     x28, #0x2828
    movz    x9, #0xbeef
    fmov    d8, x9
    mov     x9, #0x5a5a
    dup     v9.2d, x9
    mov     x7, x5                       // out pointer survives in x7
    mov     x8, #220
    LX_SVC
    cbz     x0, 1f
    ldp     d8, d9, [sp, #80]
    ldp     x27, x28, [sp, #64]
    ldp     x25, x26, [sp, #48]
    ldp     x23, x24, [sp, #32]
    ldp     x21, x22, [sp, #16]
    ldp     x19, x20, [sp], #96
    ret
1:  // child, on the new stack: record x0, x19, x20, x28, d8, v9, sp
    str     x0, [x7, #0]
    stp     x19, x20, [x7, #8]
    str     x28, [x7, #24]
    fmov    x9, d8
    str     x9, [x7, #32]
    mov     x9, v9.d[1]
    str     x9, [x7, #40]
    mov     x9, sp
    str     x9, [x7, #48]
    mov     x0, #0
    mov     x8, #93                      // __NR_exit
    LX_SVC
    brk     #1                           // not reached
