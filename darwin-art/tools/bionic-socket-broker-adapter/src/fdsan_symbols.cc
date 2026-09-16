#include "fdsan_symbols.h"

#include "fdsan.h"

#include <cstring>

extern "C" DarwinArtBionicSocketBrokerFunction
darwin_art_bionic_socket_broker_fdsan_resolve(const char* soname,
                                              const char* symbol,
                                              const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, "libc.so") != 0) {
    return nullptr;
  }

  if (std::strcmp(symbol, "android_fdsan_get_error_level") == 0) {
    if (std::strcmp(version, "LIBC_Q") != 0) return nullptr;
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_fdsan_get_error_level);
  }
  if (std::strcmp(symbol, "android_fdsan_get_owner_tag") == 0) {
    if (std::strcmp(version, "LIBC_Q") != 0) return nullptr;
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_fdsan_get_owner_tag);
  }
  if (std::strcmp(symbol, "android_fdsan_set_error_level") == 0) {
    if (std::strcmp(version, "LIBC_Q") != 0) return nullptr;
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_fdsan_set_error_level);
  }
  if (std::strcmp(version, "LIBC") != 0 &&
      std::strcmp(version, "LIBC_Q") != 0) {
    return nullptr;
  }
  if (std::strcmp(symbol, "android_fdsan_close_with_tag") == 0) {
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_android_fdsan_close_with_tag);
  }
  if (std::strcmp(symbol, "android_fdsan_create_owner_tag") == 0) {
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_android_fdsan_create_owner_tag);
  }
  if (std::strcmp(symbol, "android_fdsan_exchange_owner_tag") == 0) {
    return reinterpret_cast<DarwinArtBionicSocketBrokerFunction>(
        &darwin_art_bionic_android_fdsan_exchange_owner_tag);
  }
  return nullptr;
}
