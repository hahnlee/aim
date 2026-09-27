// Fake guest code for the signal experiment. Each probe parks known values
// in registers, faults (or spins) at an exported label, and reports what it
// sees after the guest handler resumed it.
#include "lx.h"

    .text
    .p2align 4

// bionic's __restore_rt: the sa_restorer every bionic handler gets.
    .globl _guest_restore_rt
_guest_restore_rt:
    mov     x8, #139                     // __NR_rt_sigreturn
    LX_SVC
    brk     #0x1                         // not reached

// uint64_t guest_null_probe(uint64_t out[4])
// Like ART compiled code doing an implicit null check: ldr from [null + 8].
// A guest handler is expected to redirect pc to guest_npe_landing.
    .globl _guest_null_probe, _guest_null_fault_pc, _guest_npe_landing
_guest_null_probe:
    stp     x19, x20, [sp, #-48]!
    stp     x29, x30, [sp, #16]
    stp     d8, d9, [sp, #32]
    mov     x20, x0                      // out
    mov     x19, #0x1919
    movz    x9, #0xcafe
    fmov    d8, x9                       // v8 = 0xcafe
    mov     x0, #0
_guest_null_fault_pc:
    ldr     w1, [x0, #8]
    // Falling through here means the handler just skipped the load.
    mov     x0, #0
    b       1f
_guest_npe_landing:
    str     x1, [x20, #0]                // handler-written x1
    str     x19, [x20, #8]               // handler-written x19
    fmov    x9, d8
    str     x9, [x20, #16]               // handler-written v8
    mov     x0, #1
1:  ldp     d8, d9, [sp, #32]
    ldp     x29, x30, [sp, #16]
    ldp     x19, x20, [sp], #48
    ret

// uint64_t guest_store_probe(void *addr): str to addr; handler skips it and
// sets x0.
    .globl _guest_store_probe, _guest_store_fault_pc
_guest_store_probe:
_guest_store_fault_pc:
    str     xzr, [x0]
    ret

// uint64_t guest_load_probe(void *addr)
    .globl _guest_load_probe, _guest_load_fault_pc
_guest_load_probe:
_guest_load_fault_pc:
    ldr     x0, [x0]
    ret

// uint64_t guest_spin_probe(uint64_t out[2]):
// fill x19-x28 and v8-v15 with patterns, spin until x10 != 0 (only a signal
// handler can change it), then check every pattern survived.
// out[0] = iterations, returns a bitmap of corrupted registers (0 = none).
    .globl _guest_spin_probe, _guest_spin_loop
_guest_spin_probe:
    stp     x19, x20, [sp, #-128]!
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     d8, d9, [sp, #80]
    stp     d10, d11, [sp, #96]
    stp     d12, d13, [sp, #112]
    str     x0, [sp, #-16]!
    mov     x19, #0x19
    mov     x20, #0x20
    mov     x21, #0x21
    mov     x22, #0x22
    mov     x23, #0x23
    mov     x24, #0x24
    mov     x25, #0x25
    mov     x26, #0x26
    mov     x27, #0x27
    mov     x28, #0x28
    mov     x9, #0x808
    dup     v8.2d, x9
    mov     x9, #0x909
    dup     v9.2d, x9
    mov     x9, #0xf0f
    dup     v15.2d, x9
    mov     x9, #0
    mov     x10, #0
_guest_spin_loop:
    add     x9, x9, #1
    cbz     x10, _guest_spin_loop
    ldr     x0, [sp], #16
    str     x9, [x0]
    mov     x0, #0
    cmp     x19, #0x19
    cset    x1, ne
    orr     x0, x0, x1
    cmp     x20, #0x20
    cset    x1, ne
    orr     x0, x0, x1, lsl #1
    cmp     x28, #0x28
    cset    x1, ne
    orr     x0, x0, x1, lsl #2
    mov     x1, v8.d[1]
    cmp     x1, #0x808
    cset    x1, ne
    orr     x0, x0, x1, lsl #3
    mov     x1, v9.d[0]
    cmp     x1, #0x909
    cset    x1, ne
    orr     x0, x0, x1, lsl #4
    mov     x1, v15.d[1]
    cmp     x1, #0xf0f
    cset    x1, ne
    orr     x0, x0, x1, lsl #5
    ldp     d12, d13, [sp, #112]
    ldp     d10, d11, [sp, #96]
    ldp     d8, d9, [sp, #80]
    ldp     x27, x28, [sp, #64]
    ldp     x25, x26, [sp, #48]
    ldp     x23, x24, [sp, #32]
    ldp     x21, x22, [sp, #16]
    ldp     x19, x20, [sp], #128
    ret

// uint64_t guest_syscall_storm(uint64_t n): n x gettid through the svc
// thunk with patterns in callee-saved, x16/x17-adjacent and SIMD registers;
// returns the number of iterations on which any pattern was wrong.
// Signals hammer this thread meanwhile, landing in guest code, the thunk,
// the entry/exit trampolines and the dispatcher.
    .globl _guest_syscall_storm
_guest_syscall_storm:
    stp     x19, x20, [sp, #-96]!
    stp     x21, x22, [sp, #16]
    stp     x23, x24, [sp, #32]
    stp     x25, x26, [sp, #48]
    stp     x27, x28, [sp, #64]
    stp     d8, d9, [sp, #80]
    mov     x19, x0                      // remaining
    mov     x20, #0                      // bad iterations
    mov     x21, #0x212
    mov     x28, #0x282
    mov     x9, #0x080
    dup     v8.2d, x9
1:  mov     x15, #0x151
    mov     x7, #0x070
    mov     x8, #178                     // gettid
    LX_SVC
    mov     x9, #0
    cmp     x21, #0x212
    cinc    x9, x9, ne
    cmp     x28, #0x282
    cinc    x9, x9, ne
    cmp     x15, #0x151
    cinc    x9, x9, ne
    cmp     x7, #0x070
    cinc    x9, x9, ne
    mov     x10, v8.d[1]
    cmp     x10, #0x080
    cinc    x9, x9, ne
    cmp     x9, #0
    cinc    x20, x20, ne
    subs    x19, x19, #1
    b.ne    1b
    mov     x0, x20
    ldp     d8, d9, [sp, #80]
    ldp     x27, x28, [sp, #64]
    ldp     x25, x26, [sp, #48]
    ldp     x23, x24, [sp, #32]
    ldp     x21, x22, [sp, #16]
    ldp     x19, x20, [sp], #96
    ret

// void lx_enter_handler(const uint64_t e[7]) : pc, sp, x0, x1, x2, x29, x30
// Host-side jump into a guest signal handler at syscall exit. x16/x17 are
// free at a function entry, so no thunk tail is needed.
    .globl _lx_enter_handler, _lx_enter_handler_end
_lx_enter_handler:
    mov     x16, x0
    ldr     x9, [x16, #8]
    mov     sp, x9
    ldp     x0, x1, [x16, #16]
    ldr     x2, [x16, #32]
    ldp     x29, x30, [x16, #40]
    ldr     x16, [x16, #0]
    br      x16
_lx_enter_handler_end:

// Resume trap: rt_sigreturn restores an arbitrary context by faulting here
// and letting the host SIGILL handler overwrite the Darwin mcontext.
    .globl _lx_resume_trap
_lx_resume_trap:
    udf     #0x4c58
