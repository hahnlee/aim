#pragma once
#include <cstdint>
extern "C" void darwin_art_fdsan_initialize();
// Internal process policy ABI. Public Android property/SDK routing is separate.
extern "C" int darwin_art_fdsan_get_error_level();
extern "C" uint64_t darwin_art_fdsan_get_owner_tag(int);
extern "C" int darwin_art_fdsan_set_error_level(int);
extern "C" int darwin_art_bionic_android_fdsan_set_error_level_from_property(int);
extern "C" uint64_t darwin_art_bionic_android_fdsan_create_owner_tag(int, uint64_t);
extern "C" void darwin_art_bionic_android_fdsan_exchange_owner_tag(int, uint64_t, uint64_t);
extern "C" int darwin_art_bionic_android_fdsan_close_with_tag(int, uint64_t);
