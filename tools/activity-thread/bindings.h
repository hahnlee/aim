#ifndef AIM_ACTIVITY_THREAD_MALLOPT_BINDINGS_H_
#define AIM_ACTIVITY_THREAD_MALLOPT_BINDINGS_H_

/*
 * ActivityThread is imported from AOSP unchanged. These preprocessor
 * bindings keep its allocator calls on aim's explicit provider ABI;
 * they must not resolve to the host libc mallopt symbols.
 */
#include "aim_bionic_allocator.h"

#define mallopt aim_bionic_mallopt
#define android_mallopt aim_bionic_android_mallopt

#endif  // AIM_ACTIVITY_THREAD_MALLOPT_BINDINGS_H_
