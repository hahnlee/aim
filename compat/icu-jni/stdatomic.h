#ifndef AIM_COMPAT_ICU_JNI_STDATOMIC_H_
#define AIM_COMPAT_ICU_JNI_STDATOMIC_H_

#if defined(__cplusplus)
// cutils/trace.h only needs this C11 declaration. Xcode 26's stdatomic.h
// intentionally conflicts with libc++ <atomic> before C++23, while Android's
// platform headers allow the two to coexist under the module's C++ standard.
typedef _Atomic(bool) atomic_bool;
#else
#include_next <stdatomic.h>
#endif

#endif  // AIM_COMPAT_ICU_JNI_STDATOMIC_H_
