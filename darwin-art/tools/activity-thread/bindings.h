#ifndef DARWIN_ART_ACTIVITY_THREAD_MALLOPT_BINDINGS_H_
#define DARWIN_ART_ACTIVITY_THREAD_MALLOPT_BINDINGS_H_

/*
 * ActivityThread is imported from AOSP unchanged. These preprocessor
 * bindings keep its allocator calls on Darwin ART's explicit provider ABI;
 * they must not resolve to the host libc mallopt symbols.
 */
#include "darwin_art_bionic_allocator.h"

#define mallopt darwin_art_bionic_mallopt
#define android_mallopt darwin_art_bionic_android_mallopt

#endif  // DARWIN_ART_ACTIVITY_THREAD_MALLOPT_BINDINGS_H_
