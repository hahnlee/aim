// A native library that tests/ndk.rs stores in an APK (t_apklib.c). Its
// code has sites to rewrite, a raw syscall and a thread-local variable
// (`mrs tpidr_el0`).
#include <sys/syscall.h>

static __thread long calls;

long apk_lib_tid(void) {
  register long x8 __asm__("x8") = __NR_gettid;
  register long x0 __asm__("x0");
  __asm__ volatile("svc #0" : "=r"(x0) : "r"(x8) : "memory");
  calls++;
  return x0;
}

long apk_lib_calls(void) {
  return calls;
}
