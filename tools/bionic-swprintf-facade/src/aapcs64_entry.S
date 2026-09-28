.text
.p2align 2
.globl _aim_bionic_swprintf
_aim_bionic_swprintf:
  sub sp, sp, #176
  stp q0, q1, [sp, #0]
  stp q2, q3, [sp, #32]
  stp q4, q5, [sp, #64]
  stp q6, q7, [sp, #96]
  str x30, [sp, #128]
  add x3, sp, #0
  add x4, sp, #176
  bl _aim_bionic_swprintf_captured
  ldr x30, [sp, #128]
  add sp, sp, #176
  ret
