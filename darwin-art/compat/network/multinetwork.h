#pragma once

#include <cstdint>

// Closed libandroid.so owner for Android's multinetwork C ABI. The resolver
// accepts only the public unversioned ABI and Android's LIBANDROID version.
extern "C" void* darwin_art_android_multinetwork_symbol(
    const char* symbol, const char* version);
