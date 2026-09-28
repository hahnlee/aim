#ifndef AIM_BIONIC_SOCKET_BROKER_FDSAN_SYMBOLS_H_
#define AIM_BIONIC_SOCKET_BROKER_FDSAN_SYMBOLS_H_

#include "../include/aim_bionic_socket_broker.h"

/* Resolve the Android fdsan ABI owned by the fdsan severity module. */
extern "C" {
AimBionicSocketBrokerFunction
aim_bionic_socket_broker_fdsan_resolve(const char* soname,
                                              const char* symbol,
                                              const char* version);
}

#endif  // AIM_BIONIC_SOCKET_BROKER_FDSAN_SYMBOLS_H_
