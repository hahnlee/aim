#ifndef AIM_DNS_HINTS_H_
#define AIM_DNS_HINTS_H_
#include <netdb.h>
#include <sys/socket.h>

// Android bionic's netdb values are BSD-derived, not glibc values. Keep the
// explicit mapping even where Darwin currently happens to use the same bit.
inline int AimTranslateDnsHints(addrinfo* hints) {
  switch (hints->ai_family) {
    case 0: hints->ai_family = AF_UNSPEC; break;
    case 2: hints->ai_family = AF_INET; break;
    case 10: hints->ai_family = AF_INET6; break;
    default: return EAI_FAMILY;
  }
  const int android_flags = hints->ai_flags;
  if ((android_flags & ~0xd0f) != 0) return EAI_BADFLAGS;
  int flags = 0;
  if (android_flags & 1) flags |= AI_PASSIVE;
  if (android_flags & 2) flags |= AI_CANONNAME;
  if (android_flags & 4) flags |= AI_NUMERICHOST;
  if (android_flags & 8) flags |= AI_NUMERICSERV;
  if (android_flags & 0x100) flags |= AI_ALL;
  if (android_flags & 0x400) flags |= AI_ADDRCONFIG;
  if (android_flags & 0x800) flags |= AI_V4MAPPED;
  hints->ai_flags = flags;
  return 0;
}

inline int AimAndroidGaiError(int status) {
  switch (status) {
    case EAI_ADDRFAMILY: return 1;
    case EAI_AGAIN: return 2;
    case EAI_BADFLAGS: return 3;
    case EAI_FAIL: return 4;
    case EAI_FAMILY: return 5;
    case EAI_MEMORY: return 6;
    case EAI_NODATA: return 7;
    case EAI_NONAME: return 8;
    case EAI_SERVICE: return 9;
    case EAI_SOCKTYPE: return 10;
    case EAI_SYSTEM: return 11;
    case EAI_BADHINTS: return 12;
    case EAI_PROTOCOL: return 13;
    case EAI_OVERFLOW: return 14;
    default: return 4;
  }
}
#endif
