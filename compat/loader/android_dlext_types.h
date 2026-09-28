#pragma once
#include <cstdint>
#include <sys/types.h>

// Preserve the original Bionic ABI on Darwin, whose off_t is already 64-bit.
using off64_t = int64_t;
static_assert(sizeof(off_t) == sizeof(off64_t));
#include <android/dlext.h>
#include <cstddef>
static_assert(sizeof(android_dlextinfo) == 48);
static_assert(offsetof(android_dlextinfo, library_fd_offset) == 32);
static_assert(offsetof(android_dlextinfo, library_namespace) == 40);
