#ifndef DARWIN_ART_BIONIC_SOCKET_BROKER_FDSAN_SYMBOLS_H_
#define DARWIN_ART_BIONIC_SOCKET_BROKER_FDSAN_SYMBOLS_H_

#include "../include/darwin_art_bionic_socket_broker.h"

/* Resolve the Android fdsan ABI owned by the fdsan severity module. */
extern "C" {
DarwinArtBionicSocketBrokerFunction
darwin_art_bionic_socket_broker_fdsan_resolve(const char* soname,
                                              const char* symbol,
                                              const char* version);
}

#endif  // DARWIN_ART_BIONIC_SOCKET_BROKER_FDSAN_SYMBOLS_H_
