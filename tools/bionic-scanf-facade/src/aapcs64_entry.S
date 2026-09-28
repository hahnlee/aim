.text
.p2align 2
.globl _aim_bionic_sscanf
_aim_bionic_sscanf:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  add x2, sp, #0
  add x3, sp, #64
  add x4, sp, #224
  bl _aim_bionic_sscanf_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret

.p2align 2
.globl _aim_bionic_fscanf
_aim_bionic_fscanf:
  sub sp, sp, #224
  stp x0, x1, [sp, #0]
  stp x2, x3, [sp, #16]
  stp x4, x5, [sp, #32]
  stp x6, x7, [sp, #48]
  stp q0, q1, [sp, #64]
  stp q2, q3, [sp, #96]
  stp q4, q5, [sp, #128]
  stp q6, q7, [sp, #160]
  str x30, [sp, #192]
  ldp x0, x1, [sp, #0]
  add x2, sp, #0
  add x3, sp, #64
  add x4, sp, #224
  bl _aim_bionic_fscanf_captured
  ldr x30, [sp, #192]
  add sp, sp, #224
  ret
