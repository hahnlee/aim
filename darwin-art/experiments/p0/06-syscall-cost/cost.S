// P0 experiment 6: cost of a redirected Linux syscall.
// Loops of n syscalls through different entry paths. All loops issue
// Linux gettid (x8 = 178), which the dispatchers answer without a kernel
// call, so the numbers are pure redirection overhead.
#include "lx.h"

    .macro THUNK entry
    stp     x16, x17, [sp, #-32]!
    str     x30, [sp, #16]
    bl      \entry
    ldr     x30, [sp, #16]
    ldp     x16, x17, [sp], #32
    .endm

    .text
    .p2align 4

    .globl _loop_call
_loop_call:                      // baseline: bl/ret to an empty function
    stp     x29, x30, [sp, #-16]!
    str     x19, [sp, #-16]!
    mov     x19, x0
1:  bl      _empty
    subs    x19, x19, #1
    b.ne    1b
    ldr     x19, [sp], #16
    ldp     x29, x30, [sp], #16
    ret
_empty:
    ret

    .globl _loop_darwin_getpid
_loop_darwin_getpid:             // native Darwin syscall, raw svc #0x80
    mov     x9, x0
1:  mov     x16, #20             // SYS_getpid
    svc     #0x80
    subs    x9, x9, #1
    b.ne    1b
    ret

    .globl _loop_darwin_getppid
_loop_darwin_getppid:
    mov     x9, x0
1:  mov     x16, #39             // SYS_getppid
    svc     #0x80
    subs    x9, x9, #1
    b.ne    1b
    ret

    .globl _loop_thunk_only
_loop_thunk_only:                // thunk shape with an entry that just returns
    stp     x29, x30, [sp, #-16]!
    mov     x9, x0
1:  mov     x8, #178
    THUNK   _entry_ret
    subs    x9, x9, #1
    b.ne    1b
    ldp     x29, x30, [sp], #16
    ret
_entry_ret:
    ret

    .globl _loop_full
_loop_full:                      // common/lx_trampoline.S: x0-x30, q0-q31, nzcv, fpsr, fpcr
    stp     x29, x30, [sp, #-16]!
    str     x19, [sp, #-16]!
    mov     x19, x0
1:  mov     x8, #178
    THUNK   _lx_svc_entry
    subs    x19, x19, #1
    b.ne    1b
    ldr     x19, [sp], #16
    ldp     x29, x30, [sp], #16
    ret

    .globl _loop_int
_loop_int:
    stp     x29, x30, [sp, #-16]!
    str     x19, [sp, #-16]!
    mov     x19, x0
1:  mov     x8, #178
    THUNK   _int_entry
    subs    x19, x19, #1
    b.ne    1b
    ldr     x19, [sp], #16
    ldp     x29, x30, [sp], #16
    ret

    .globl _loop_min
_loop_min:
    stp     x29, x30, [sp, #-16]!
    str     x19, [sp, #-16]!
    mov     x19, x0
1:  mov     x8, #178
    THUNK   _min_entry
    subs    x19, x19, #1
    b.ne    1b
    ldr     x19, [sp], #16
    ldp     x29, x30, [sp], #16
    ret

// Lean path 1: every integer register (x0-x30, sp, pc, nzcv) into the
// frame, no SIMD. Valid only because lean_dispatch is compiled with
// -mgeneral-regs-only, so host code never touches v0-v31/fpsr/fpcr.
// A syscall that needs the full frame (clone, rt_sigreturn, signal
// delivery) would take the full path instead.
_int_entry:
    mrs     x16, tpidrro_el0
    adrp    x17, _lx_host_slot_off@PAGE
    ldr     x17, [x17, _lx_host_slot_off@PAGEOFF]
    ldr     x16, [x16, x17]
    stp     x0, x1, [x16, #0]
    stp     x2, x3, [x16, #16]
    stp     x4, x5, [x16, #32]
    stp     x6, x7, [x16, #48]
    stp     x8, x9, [x16, #64]
    stp     x10, x11, [x16, #80]
    stp     x12, x13, [x16, #96]
    stp     x14, x15, [x16, #112]
    ldp     x0, x1, [sp]
    stp     x0, x1, [x16, #128]
    stp     x18, x19, [x16, #144]
    stp     x20, x21, [x16, #160]
    stp     x22, x23, [x16, #176]
    stp     x24, x25, [x16, #192]
    stp     x26, x27, [x16, #208]
    stp     x28, x29, [x16, #224]
    ldr     x0, [sp, #16]
    str     x0, [x16, #240]
    add     x0, sp, #32
    stp     x0, x30, [x16, #LX_FRAME_SP]
    mrs     x0, nzcv
    str     x0, [x16, #LX_FRAME_NZCV]
    mov     x19, sp                      // x19 is callee-saved: survives the call
    ldr     x0, [x16, #LX_THREAD_HOST_SP]
    mov     sp, x0
    mov     x0, x16
    mov     x20, x16
    bl      _lean_dispatch
    mov     x16, x20
    mov     sp, x19
    ldr     x0, [x16, #LX_FRAME_NZCV]
    msr     nzcv, x0
    ldp     x0, x1, [x16, #0]
    ldp     x2, x3, [x16, #16]
    ldp     x4, x5, [x16, #32]
    ldp     x6, x7, [x16, #48]
    ldp     x8, x9, [x16, #64]
    ldp     x10, x11, [x16, #80]
    ldp     x12, x13, [x16, #96]
    ldp     x14, x15, [x16, #112]
    ldp     x18, x19, [x16, #144]
    ldp     x20, x21, [x16, #160]
    ldp     x22, x23, [x16, #176]
    ldp     x24, x25, [x16, #192]
    ldp     x26, x27, [x16, #208]
    ldp     x28, x29, [x16, #224]
    ldr     x30, [x16, #LX_FRAME_PC]
    ret                                  // thunk tail restores x16/x17/x30

// Lean path 2: only what AAPCS64 lets the callee clobber: x0-x15, x18 and
// nzcv (x16/x17/x30 are already on the stack from the thunk; x19-x29 and
// sp are preserved by the C dispatcher itself). No SIMD, dispatcher built
// with -mgeneral-regs-only. x18 is saved because Darwin reserves it and a
// guest built with shadow-call-stack keeps live state in it.
_min_entry:
    mrs     x16, tpidrro_el0
    adrp    x17, _lx_host_slot_off@PAGE
    ldr     x17, [x17, _lx_host_slot_off@PAGEOFF]
    ldr     x16, [x16, x17]
    stp     x0, x1, [x16, #0]
    stp     x2, x3, [x16, #16]
    stp     x4, x5, [x16, #32]
    stp     x6, x7, [x16, #48]
    stp     x8, x9, [x16, #64]
    stp     x10, x11, [x16, #80]
    stp     x12, x13, [x16, #96]
    stp     x14, x15, [x16, #112]
    str     x18, [x16, #144]
    mrs     x9, nzcv
    str     x9, [x16, #LX_FRAME_NZCV]
    stp     x19, x20, [x16, #152]        // scratch slots for our own callee-saved use
    str     x30, [x16, #LX_FRAME_PC]
    mov     x19, sp
    mov     x20, x16
    ldr     x9, [x16, #LX_THREAD_HOST_SP]
    mov     sp, x9
    mov     x0, x16
    bl      _lean_dispatch
    mov     x16, x20
    mov     sp, x19
    ldp     x19, x20, [x16, #152]
    ldr     x9, [x16, #LX_FRAME_NZCV]
    msr     nzcv, x9
    ldp     x0, x1, [x16, #0]
    ldp     x2, x3, [x16, #16]
    ldp     x4, x5, [x16, #32]
    ldp     x6, x7, [x16, #48]
    ldp     x8, x9, [x16, #64]
    ldp     x10, x11, [x16, #80]
    ldp     x12, x13, [x16, #96]
    ldp     x14, x15, [x16, #112]
    ldr     x18, [x16, #144]
    ldr     x30, [x16, #LX_FRAME_PC]
    ret

// SIGSYS route: execute a raw `svc #0` with an invalid Darwin number in x16
// and let a SIGSYS handler emulate. Shown for its cost only: a real guest's
// x16 at an svc is arbitrary and may name a valid Darwin syscall.
    .globl _loop_sigsys
_loop_sigsys:
    mov     x9, x0
1:  mov     x8, #178
    mov     x16, #0x3ff              // not a Darwin syscall
    svc     #0
    subs    x9, x9, #1
    b.ne    1b
    ret
