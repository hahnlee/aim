.text
.p2align 2

/* Android: long double strtold(const char*, char**). Darwin never observes a
 * long-double type. The core writes IEEE binary128 to our aligned spill and
 * this entry returns those raw bits in q0 as required by AAPCS64. */
.globl _aim_bionic_strtold
_aim_bionic_strtold:
  stp x29, x30, [sp, #-32]!
  mov x29, sp
  add x2, sp, #16
  bl _aim_bionic_strtold_raw
  ldr q0, [sp, #16]
  ldp x29, x30, [sp], #32
  ret

.globl _aim_bionic_strtold_l
_aim_bionic_strtold_l:
  stp x29, x30, [sp, #-32]!
  mov x29, sp
  add x3, sp, #16
  bl _aim_bionic_strtold_l_raw
  ldr q0, [sp, #16]
  ldp x29, x30, [sp], #32
  ret

.globl _aim_bionic_wcstold
_aim_bionic_wcstold:
  stp x29, x30, [sp, #-32]!
  mov x29, sp
  add x2, sp, #16
  bl _aim_bionic_wcstold_raw
  ldr q0, [sp, #16]
  ldp x29, x30, [sp], #32
  ret

/* Android binary128 powl(q0, q1) -> q0. Keep Darwin's binary64 long double
 * ABI completely outside the guest boundary. */
.globl _aim_bionic_powl
_aim_bionic_powl:
  stp x29, x30, [sp, #-64]!
  mov x29, sp
  str q0, [sp, #16]
  str q1, [sp, #32]
  add x0, sp, #16
  add x1, sp, #32
  add x2, sp, #48
  bl _aim_bionic_powl_raw
  ldr q0, [sp, #48]
  ldp x29, x30, [sp], #64
  ret
