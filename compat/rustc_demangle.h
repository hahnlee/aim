#pragma once

#include <cstddef>

// Implemented by the Rust provider linked into the production unwind closure.
// Its result uses libc allocation, matching AOSP Demangle.cpp's free() call.
extern "C" char* rustc_demangle(const char*, char*, size_t*, int*);
