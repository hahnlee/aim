#include "../compat/darwin_dns_hints.h"
#include <cassert>
#include <cstdio>
int main() {
  addrinfo hints{};
  hints.ai_family = 10;
  hints.ai_flags = 0x400;
  assert(AimTranslateDnsHints(&hints) == 0);
  assert(hints.ai_family == AF_INET6 && hints.ai_flags == AI_ADDRCONFIG);
  hints = {};
  hints.ai_family = 2;
  hints.ai_flags = 4;
  assert(AimTranslateDnsHints(&hints) == 0);
  addrinfo* result = nullptr;
  assert(getaddrinfo("127.0.0.1", nullptr, &hints, &result) == 0);
  freeaddrinfo(result);
  hints = {};
  hints.ai_family = 10;
  hints.ai_flags = 4;
  assert(AimTranslateDnsHints(&hints) == 0);
  result = nullptr;
  assert(getaddrinfo("::1", nullptr, &hints, &result) == 0);
  assert(result != nullptr && result->ai_family == AF_INET6);
  freeaddrinfo(result);
  hints = {};
  hints.ai_family = 30;
  assert(AimTranslateDnsHints(&hints) == EAI_FAMILY);
  hints = {};
  hints.ai_flags = 0x20; // glibc AI_ADDRCONFIG is not Android's contract.
  assert(AimTranslateDnsHints(&hints) == EAI_BADFLAGS);
  assert(AimAndroidGaiError(EAI_NONAME) == 8);
  assert(AimAndroidGaiError(EAI_SYSTEM) == 11);
  std::puts("dns-hints: Android IPv4/IPv6/flags and invalid input PASS");
}
