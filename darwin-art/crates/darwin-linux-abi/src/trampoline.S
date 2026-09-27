// Guest <-> host transitions for the Linux syscall layer (Darwin arm64).
//
// GuestContext layout (must match src/context.rs):
//   0    x0..x30   (31 * 8)
//   248  sp
//   256  pc
//   264  nzcv
//   272  fpcr
//   280  fpsr
//   288  stub_ret   (return address into the per-site stub)
//   296  host_sp    (top of this thread's host stack)
//   320  v0..v31    (32 * 16)
//
// x18 is never used as a scratch register: Android's shadow call stack owns it.

    .text

// Syscall entry, reached from a per-site stub in a trampoline island:
//
//   stp  x16, x17, [sp, #-32]!
//   str  x30, [sp, #16]
//   ldr  x16, <island literal: _linux_abi_syscall_entry>
//   blr  x16
//   ldr  x30, [sp, #16]
//   ldp  x16, x17, [sp], #32
//   b    <svc site + 4>
//
// On entry: [sp] = guest x16, [sp+8] = guest x17, [sp+16] = guest x30,
// guest sp = sp + 32, x30 = return address into the stub.
// The GuestContext of this thread lives in a Darwin TSD slot (TPIDRRO_EL0 + offset).
    .globl _linux_abi_syscall_entry
    .p2align 4
_linux_abi_syscall_entry:
    mrs     x16, tpidrro_el0
    adrp    x17, _LINUX_ABI_CTX_TSD_OFFSET@PAGE
    ldr     x17, [x17, _LINUX_ABI_CTX_TSD_OFFSET@PAGEOFF]
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
    str     x0, [x16, #248]
    str     x30, [x16, #288]
    mrs     x0, nzcv
    str     x0, [x16, #264]
    mrs     x0, fpcr
    str     x0, [x16, #272]
    mrs     x0, fpsr
    str     x0, [x16, #280]
    add     x0, x16, #320
    stp     q0, q1, [x0, #0]
    stp     q2, q3, [x0, #32]
    stp     q4, q5, [x0, #64]
    stp     q6, q7, [x0, #96]
    stp     q8, q9, [x0, #128]
    stp     q10, q11, [x0, #160]
    stp     q12, q13, [x0, #192]
    stp     q14, q15, [x0, #224]
    stp     q16, q17, [x0, #256]
    stp     q18, q19, [x0, #288]
    stp     q20, q21, [x0, #320]
    stp     q22, q23, [x0, #352]
    stp     q24, q25, [x0, #384]
    stp     q26, q27, [x0, #416]
    stp     q28, q29, [x0, #448]
    stp     q30, q31, [x0, #480]

    // Switch to the host stack and dispatch. x19 is callee-saved in the host ABI.
    mov     x19, x16
    ldr     x0, [x16, #296]
    mov     sp, x0
    mov     x29, #0
    mov     x0, x19
    bl      _linux_abi_dispatch

    mov     x16, x19
    add     x0, x16, #320
    ldp     q0, q1, [x0, #0]
    ldp     q2, q3, [x0, #32]
    ldp     q4, q5, [x0, #64]
    ldp     q6, q7, [x0, #96]
    ldp     q8, q9, [x0, #128]
    ldp     q10, q11, [x0, #160]
    ldp     q12, q13, [x0, #192]
    ldp     q14, q15, [x0, #224]
    ldp     q16, q17, [x0, #256]
    ldp     q18, q19, [x0, #288]
    ldp     q20, q21, [x0, #320]
    ldp     q22, q23, [x0, #352]
    ldp     q24, q25, [x0, #384]
    ldp     q26, q27, [x0, #416]
    ldp     q28, q29, [x0, #448]
    ldp     q30, q31, [x0, #480]
    ldr     x0, [x16, #264]
    msr     nzcv, x0
    ldr     x0, [x16, #272]
    msr     fpcr, x0
    ldr     x0, [x16, #280]
    msr     fpsr, x0
    // Rebuild the stub frame below the (possibly updated) guest sp; the stub
    // pops x16, x17 and x30 from it.
    ldr     x0, [x16, #248]
    sub     x0, x0, #32
    mov     sp, x0
    ldp     x1, x2, [x16, #128]
    stp     x1, x2, [sp]
    ldr     x1, [x16, #240]
    str     x1, [sp, #16]
    ldr     x30, [x16, #288]
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
    ret

// void linux_abi_enter_guest(const GuestContext *ctx)  -- never returns.
// Loads the full register state and branches to ctx->pc. x17 carries the
// branch target, so the guest starts with x17 == pc (Linux leaves it unspecified).
    .globl _linux_abi_enter_guest
    .p2align 4
_linux_abi_enter_guest:
    mov     x16, x0
    add     x0, x16, #320
    ldp     q0, q1, [x0, #0]
    ldp     q2, q3, [x0, #32]
    ldp     q4, q5, [x0, #64]
    ldp     q6, q7, [x0, #96]
    ldp     q8, q9, [x0, #128]
    ldp     q10, q11, [x0, #160]
    ldp     q12, q13, [x0, #192]
    ldp     q14, q15, [x0, #224]
    ldp     q16, q17, [x0, #256]
    ldp     q18, q19, [x0, #288]
    ldp     q20, q21, [x0, #320]
    ldp     q22, q23, [x0, #352]
    ldp     q24, q25, [x0, #384]
    ldp     q26, q27, [x0, #416]
    ldp     q28, q29, [x0, #448]
    ldp     q30, q31, [x0, #480]
    ldr     x0, [x16, #264]
    msr     nzcv, x0
    ldr     x0, [x16, #272]
    msr     fpcr, x0
    ldr     x0, [x16, #248]
    mov     sp, x0
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
    ldr     x30, [x16, #240]
    ldr     x17, [x16, #256]
    ldr     x16, [x16, #128]
    br      x17
