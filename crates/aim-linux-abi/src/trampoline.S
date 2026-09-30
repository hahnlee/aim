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
//   304  tid        (Linux tid)
//   312  in_host    (u32: host code runs for this thread)
//   320  v0..v31    (32 * 16)
//   {attn}  attn       (pointer to the thread's signal attention flag)
//   {orig}  orig_x0    (x0 of a lean-path syscall)
//
// x18 is never used: Android's shadow call stack owns it on Linux.
//
// Everything is one atom: the other global labels are `.alt_entry`, so the
// linker neither strips nor reorders them (context::region relies on the
// order).
//
// Signals (see src/sys/signal.rs). Every syscall exit checks the thread's
// attention flag and, when set, calls into Rust to deliver. A host signal
// handler that interrupts the trampoline acts by region (context::region):
// in an exit check it restarts the check; elsewhere it leaves the flag for
// the check that follows.

    .text

// Syscall entry, reached from a per-site stub (in a translated file's stub
// segment or a load-time island):
//
//   stp  x16, x17, [sp, #-32]!
//   str  x30, [sp, #16]
//   mrs  x16, tpidrro_el0
//   ldr  x16, [x16, #ENTRY slot]
//   blr  x16
//   ldr  x30, [sp, #16]          <- x30 (stub_ret) points here
//   ldp  x16, x17, [sp], #32
//   b    <svc site + 4>
//
// On entry: [sp] = guest x16, [sp+8] = guest x17, [sp+16] = guest x30,
// guest sp = sp + 32. The thread's GuestContext is in a fixed TSD slot.
//
// Two paths (host calls take a third, Lhostcall below):
// - lean: the syscalls below are answered right here, on live registers,
//   with at most one Darwin syscall. Only x16/x17 (already saved by the
//   stub) are scratch, NZCV is kept in the free slot at [sp+24], and no host
//   code runs, so no other integer, FP/SIMD or FPCR/FPSR register is
//   touched. Darwin's `svc #0x80` returns in x0/x1 and the carry flag; x1 is
//   kept in x17 across it.
// - full: every register (FP/SIMD too) goes into the GuestContext, then
//   the Rust dispatcher runs on the host stack. Host code may use any
//   caller-saved vector register, and the guest must see them preserved.
    .globl _linux_abi_syscall_entry
    .p2align 4
_linux_abi_syscall_entry:
    mrs     x17, nzcv
    str     x17, [sp, #24]
    adrp    x16, {trace}@PAGE
    ldrb    w16, [x16, {trace}@PAGEOFF]
    cbnz    w16, Lfull                  // --trace: everything through Rust
    cmp     x8, #172
    b.eq    Lgetpid
    cmp     x8, #178
    b.eq    Lgettid
    cmp     x8, #63
    b.eq    Lread
    cmp     x8, #64
    b.eq    Lwrite
    cmp     x8, #67
    b.eq    Lpread
    cmp     x8, #68
    b.eq    Lpwrite
    cmp     x8, #57
    b.eq    Lclose
    cmp     x8, #124
    b.eq    Lyield
    sub     x16, x8, #174               // getuid, geteuid, getgid, getegid
    cmp     x16, #3
    b.ls    Lids
    movz    x16, #{hostcall_hi}, lsl #16
    cmp     x8, x16
    b.eq    Lhostcall
    b       Lfull

Lgetpid:
    adrp    x16, {pid}@PAGE
    ldr     x0, [x16, {pid}@PAGEOFF]
    b       Llean_ret
Lgettid:
    mrs     x16, tpidrro_el0
    ldr     x16, [x16, #{ctx}]
    ldr     x0, [x16, #304]
    b       Llean_ret
Lread:
    mov     x16, #3
    b       Lfd_darwin
Lpread:
    mov     x16, #153
    b       Lfd_darwin
// Writes are charged to the process's budget of unchecked bytes
// (sys/space.rs); past it, they take the full path, which looks at the
// volume's free space.
Lwrite:
Lpwrite:
    adrp    x16, {budget}@PAGE
    ldr     x17, [x16, {budget}@PAGEOFF]
    subs    x17, x17, x2
    b.lo    Lfull
    str     x17, [x16, {budget}@PAGEOFF]
    mov     x16, #4                     // write
    cmp     x8, #64
    b.eq    Lfd_darwin
    mov     x16, #154                   // pwrite
    b       Lfd_darwin
Lclose:
    mov     x16, #6
    b       Lfd_darwin
Lids:                                   // the guest's ids (sys/cred.rs)
    adrp    x17, {ids}@PAGE
    add     x17, x17, {ids}@PAGEOFF
    ldr     x0, [x17, x16, lsl #3]
    b       Llean_ret
Lyield:
    mov     x17, x1
    mov     x16, #-59                   // swtch_pri mach trap
    mov     x0, #0
    svc     #0x80
    mov     x1, x17
    mov     x0, #0
    b       Llean_ret

// An fd call: fds with Linux state beyond the host object (eventfd,
// SEQPACKET sockets, epoll, ...; see sys/fdtab.rs) and fds past the table
// take the full path.
Lfd_darwin:
    lsr     x17, x0, #16
    cbnz    x17, Lfull
    adrp    x17, {slow}@PAGE
    add     x17, x17, {slow}@PAGEOFF
    ldrb    w17, [x17, x0]
    cbnz    w17, Lfull
    // fall through

// One Darwin BSD syscall on the guest's live x0..x3; carry set means errno.
// x0 is kept for a restart after a signal.
Ldarwin:
    mrs     x17, tpidrro_el0
    ldr     x17, [x17, #{ctx}]
    str     x0, [x17, #{orig}]
    mov     x17, x1
    svc     #0x80
    mov     x1, x17
    b.cc    Llean_ret
    cmp     x0, #128
    b.hs    1f
    adrp    x16, {errtab}@PAGE
    add     x16, x16, {errtab}@PAGEOFF
    ldrb    w0, [x16, x0]
    neg     x0, x0
    b       Llean_ret
1:  mov     x0, #-5                     // EIO beyond the table
// Lean exit check; restartable from its first instruction.
    .globl _linux_abi_lean_check
    .alt_entry _linux_abi_lean_check
_linux_abi_lean_check:
Llean_ret:
    mrs     x16, tpidrro_el0
    ldr     x16, [x16, #{ctx}]
    ldr     x16, [x16, #{attn}]
    ldr     w16, [x16]
    cbnz    w16, Llean_signal
    ldr     x17, [sp, #24]
    msr     nzcv, x17
    ret                                 // into the stub, which restores x16/x17/x30
    .globl _linux_abi_lean_end
    .alt_entry _linux_abi_lean_end
_linux_abi_lean_end:

// Host call (src/hostcall.rs): an AAPCS64 call of linux_abi_hostcall(x0..x3)
// on the host stack. Caller-saved registers are the guest's to lose, by the
// host-call contract; x19/x20 are kept below the stub frame and hold the
// guest sp and the return address into the stub across the call. It runs
// host code, so in_host is set around it; the lean exit check follows.
Lhostcall:
    stp     x19, x20, [sp, #-16]!
    mov     x19, sp
    mov     x20, x30
    mrs     x16, tpidrro_el0
    ldr     x16, [x16, #{ctx}]
    mov     w17, #1
    str     w17, [x16, #{in_host}]
    ldr     x16, [x16, #296]
    mov     sp, x16
    bl      _linux_abi_hostcall
    mrs     x16, tpidrro_el0
    ldr     x16, [x16, #{ctx}]
    str     wzr, [x16, #{in_host}]
    mov     sp, x19
    mov     x30, x20
    ldp     x19, x20, [sp], #16
    b       Llean_ret

    // ---- full path ----
    // x17 = the Rust function to call with the saved context: the
    // dispatcher, or the lean path's signal hand-off (result in x0).
Llean_signal:
    adrp    x17, _linux_abi_lean_signal@PAGE
    add     x17, x17, _linux_abi_lean_signal@PAGEOFF
    b       Lsave
Lfull:
    adrp    x17, _linux_abi_dispatch@PAGE
    add     x17, x17, _linux_abi_dispatch@PAGEOFF
Lsave:
    mrs     x16, tpidrro_el0
    ldr     x16, [x16, #{ctx}]
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
    ldr     x0, [sp, #24]
    str     x0, [x16, #264]
    // Host stack. x19 holds the context and x20 the callee; both are
    // callee-saved in the host ABI.
    mov     x19, x16
    mov     x20, x17
    mov     w0, #1
    str     w0, [x19, #{in_host}]
    ldr     x0, [x16, #296]
    mov     sp, x0
    mov     x29, #0
    mrs     x0, fpcr
    str     x0, [x19, #272]
    mrs     x0, fpsr
    str     x0, [x19, #280]
    add     x0, x19, #320
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
    mov     x0, x19
    blr     x20
// Full exit check, on the host stack with x19 = context. Everything from
// here to _linux_abi_tail_end is restarted at _linux_abi_recheck by a
// signal handler that interrupts it.
    .globl _linux_abi_check
    .alt_entry _linux_abi_check
_linux_abi_check:
    ldr     x0, [x19, #{attn}]
    ldr     w0, [x0]
    cbz     w0, Lnopending
    mov     x0, x19
    bl      _linux_abi_deliver
    b       _linux_abi_check
Lnopending:
    str     wzr, [x19, #{in_host}]
    add     x0, x19, #320
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
    ldr     x0, [x19, #272]
    msr     fpcr, x0
    ldr     x0, [x19, #280]
    msr     fpsr, x0

    mov     x16, x19
    ldr     x0, [x16, #264]
    msr     nzcv, x0
    // stub_ret == 0: enter at ctx->pc directly (a signal handler, or a
    // clone child that did not come through a stub). x17 carries the pc.
    ldr     x30, [x16, #288]
    cbz     x30, Ldirect
    // Rebuild the stub frame below the (possibly updated) guest sp; the stub
    // pops x16, x17 and x30 from it.
    ldr     x0, [x16, #248]
    sub     x0, x0, #32
    mov     sp, x0
    ldp     x1, x2, [x16, #128]
    stp     x1, x2, [sp]
    ldr     x1, [x16, #240]
    str     x1, [sp, #16]
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
Ldirect:
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
// Re-enter the exit check: a signal handler sets x19 = context and sp =
// host stack before sending the thread here.
    .globl _linux_abi_recheck
    .alt_entry _linux_abi_recheck
_linux_abi_recheck:
    mov     w0, #1
    str     w0, [x19, #{in_host}]
    b       _linux_abi_check
    .globl _linux_abi_tail_end
    .alt_entry _linux_abi_tail_end
_linux_abi_tail_end:

// void linux_abi_resume(GuestContext *ctx) -- never returns. A clone child
// starts here: the exit check runs, then the copied registers are resumed.
    .globl _linux_abi_resume
    .alt_entry _linux_abi_resume
    .p2align 2
_linux_abi_resume:
    mov     x19, x0
    ldr     x0, [x19, #296]
    mov     sp, x0
    mov     x29, #0
    b       _linux_abi_recheck

// rt_sigreturn's exact resume: SIGILL here loads the whole context.
    .globl _linux_abi_resume_trap
    .alt_entry _linux_abi_resume_trap
_linux_abi_resume_trap:
    udf     #0x6c78

// void linux_abi_enter_guest(const GuestContext *ctx)  -- never returns.
// Loads the full register state and branches to ctx->pc. x17 carries the
// branch target, so the guest starts with x17 == pc (Linux leaves it unspecified).
    .globl _linux_abi_enter_guest
    .alt_entry _linux_abi_enter_guest
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
    str     wzr, [x16, #{in_host}]
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
    .globl _linux_abi_text_end
    .alt_entry _linux_abi_text_end
_linux_abi_text_end:
