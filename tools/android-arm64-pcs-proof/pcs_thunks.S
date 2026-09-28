#ifdef __APPLE__
#define PCS_SYM(name) _##name
#else
#define PCS_SYM(name) name
#endif

.text
.p2align 2

.macro PCS_SEED
  movz x15, #0xc909
  movk x15, #0xf3bc, lsl #16
  movk x15, #0xe667, lsl #32
  movk x15, #0x6a09, lsl #48
.endm

.macro PCS_MIX reg
  eor x15, x15, \reg
  ror x15, x15, #7
.endm

// Register-only signatures use the same x0-x7/v0-v7 assignment in both PCS.
.globl PCS_SYM(register_direct_target)
#ifndef __APPLE__
.type register_direct_target, %function
.hidden register_direct_target
#endif
PCS_SYM(register_direct_target):
  PCS_SEED
  PCS_MIX x0
  PCS_MIX x1
  PCS_MIX x2
  PCS_MIX x3
  fmov x9, d0
  PCS_MIX x9
  fmov x9, d1
  PCS_MIX x9
  mov x0, x15
  ret
#ifndef __APPLE__
.size register_direct_target, .-register_direct_target
#endif

// Android AAPCS64 callee. Each spilled primitive owns an eight-byte slot.
.globl PCS_SYM(android_spilled_target)
#ifndef __APPLE__
.type android_spilled_target, %function
.hidden android_spilled_target
#endif
PCS_SYM(android_spilled_target):
  PCS_SEED
  PCS_MIX x0
  PCS_MIX x1
  PCS_MIX x2
  PCS_MIX x3
  PCS_MIX x4
  PCS_MIX x5
  PCS_MIX x6
  PCS_MIX x7
  fmov x9, d0
  PCS_MIX x9
  fmov x9, d1
  PCS_MIX x9
  fmov x9, d2
  PCS_MIX x9
  fmov x9, d3
  PCS_MIX x9
  fmov x9, d4
  PCS_MIX x9
  fmov x9, d5
  PCS_MIX x9
  fmov x9, d6
  PCS_MIX x9
  fmov x9, d7
  PCS_MIX x9
  ldrb w9, [sp, #0]
  PCS_MIX x9
  ldrsb x9, [sp, #8]
  PCS_MIX x9
  ldrh w9, [sp, #16]
  PCS_MIX x9
  ldrsh x9, [sp, #24]
  PCS_MIX x9
  ldrsw x9, [sp, #32]
  PCS_MIX x9
  ldr x9, [sp, #40]
  PCS_MIX x9
  ldr x9, [sp, #48]
  PCS_MIX x9
  ldr w9, [sp, #56]
  PCS_MIX x9
  ldr x9, [sp, #64]
  PCS_MIX x9
  mov x0, x15
  ret
#ifndef __APPLE__
.size android_spilled_target, .-android_spilled_target
#endif

#ifdef __APPLE__
// Darwin compact source offsets: Z0 B1 C2 S4 I8 J16 ref24 F32 D40.
.globl PCS_SYM(darwin_to_android_thunk)
PCS_SYM(darwin_to_android_thunk):
  stp x29, x30, [sp, #-16]!
  mov x29, sp
  add x10, x29, #16
  sub sp, sp, #80
  ldrb w9, [x10, #0]
  strb w9, [sp, #0]
  ldrb w9, [x10, #1]
  strb w9, [sp, #8]
  ldrh w9, [x10, #2]
  strh w9, [sp, #16]
  ldrh w9, [x10, #4]
  strh w9, [sp, #24]
  ldr w9, [x10, #8]
  str w9, [sp, #32]
  ldr x9, [x10, #16]
  str x9, [sp, #40]
  ldr x9, [x10, #24]
  str x9, [sp, #48]
  ldr w9, [x10, #32]
  str w9, [sp, #56]
  ldr x9, [x10, #40]
  str x9, [sp, #64]
  bl PCS_SYM(android_spilled_target)
  mov sp, x29
  ldp x29, x30, [sp], #16
  ret
#endif

// Android eight-byte source slots -> Darwin compact callback stack.
.globl PCS_SYM(android_to_darwin_repack)
#ifndef __APPLE__
.type android_to_darwin_repack, %function
.hidden android_to_darwin_repack
#endif
PCS_SYM(android_to_darwin_repack):
  stp x29, x30, [sp, #-16]!
  mov x29, sp
  add x10, x29, #16
  sub sp, sp, #48
  ldrb w9, [x10, #0]
  strb w9, [sp, #0]
  ldrb w9, [x10, #8]
  strb w9, [sp, #1]
  ldrh w9, [x10, #16]
  strh w9, [sp, #2]
  ldrh w9, [x10, #24]
  strh w9, [sp, #4]
  ldr w9, [x10, #32]
  str w9, [sp, #8]
  ldr x9, [x10, #40]
  str x9, [sp, #16]
  ldr x9, [x10, #48]
  str x9, [sp, #24]
  ldr w9, [x10, #56]
  str w9, [sp, #32]
  ldr x9, [x10, #64]
  str x9, [sp, #40]
  blr x19
  mov sp, x29
  ldp x29, x30, [sp], #16
  ret
#ifndef __APPLE__
.size android_to_darwin_repack, .-android_to_darwin_repack
#endif

// A one-pointer direct entry constructs the Android caller frame.
.globl PCS_SYM(android_callback_fixture)
#ifndef __APPLE__
.type android_callback_fixture, %function
.hidden android_callback_fixture
#endif
PCS_SYM(android_callback_fixture):
  stp x29, x30, [sp, #-16]!
  mov x29, sp
  stp x19, x20, [sp, #-16]!
  mov x19, x0
  sub sp, sp, #80

  mov w9, #1
  strb w9, [sp, #0]
  mov w9, #0xfe
  strb w9, [sp, #8]
  mov w9, #0x3344
  strh w9, [sp, #16]
  mov w9, #0xfb2e
  strh w9, [sp, #24]
  movz w9, #0x2979
  movk w9, #0xffed, lsl #16
  str w9, [sp, #32]
  movz x9, #0x7788
  movk x9, #0x5566, lsl #16
  movk x9, #0x3344, lsl #32
  movk x9, #0x1122, lsl #48
  str x9, [sp, #40]
  movz x9, #0xdef0
  movk x9, #0x9abc, lsl #16
  movk x9, #0x5678, lsl #32
  movk x9, #0x1234, lsl #48
  str x9, [sp, #48]
  movz w9, #0
  movk w9, #0x3fa0, lsl #16
  str w9, [sp, #56]
  movz x9, #0
  movk x9, #0xc004, lsl #48
  str x9, [sp, #64]

  mov x0, #0x101
  mov x1, #0x202
  mov x2, #0x303
  mov x3, #0x404
  mov x4, #0x505
  mov x5, #0x606
  mov x6, #0x707
  mov x7, #0x808
  fmov d0, #1.0
  fmov d1, #2.0
  fmov d2, #3.0
  fmov d3, #4.0
  fmov d4, #5.0
  fmov d5, #6.0
  fmov d6, #7.0
  fmov d7, #8.0
  bl PCS_SYM(android_to_darwin_repack)

  add sp, sp, #80
  ldp x19, x20, [sp], #16
  ldp x29, x30, [sp], #16
  ret
#ifndef __APPLE__
.size android_callback_fixture, .-android_callback_fixture
#endif

.globl PCS_SYM(pcs_loader_smoke)
#ifndef __APPLE__
.type pcs_loader_smoke, %function
#endif
PCS_SYM(pcs_loader_smoke):
  mov w0, #42
  ret
#ifndef __APPLE__
.size pcs_loader_smoke, .-pcs_loader_smoke
#endif
