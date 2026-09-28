#include <netdb.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <stdint.h>

extern int* ImportedHerrno(void) __asm__("__get_h_errno");
extern struct hostent* ImportedHostByName(const char*) __asm__("gethostbyname");
extern struct servent* ImportedServiceByName(const char*, const char*)
    __asm__("getservbyname");

static int LegacyHostValid(const struct hostent* host) {
  if (host == (const struct hostent*)0 || host->h_name == (char*)0 ||
      host->h_addrtype != AF_INET || host->h_length != 4 ||
      host->h_addr_list == (char**)0 || host->h_addr_list[0] == (char*)0) {
    return 0;
  }
  return 1;
}

__attribute__((visibility("default"))) int DnsFixtureHostLookup(
    const char* node) {
  return LegacyHostValid(ImportedHostByName(node)) ? 42 : -1;
}

__attribute__((visibility("default"))) int DnsFixtureHostFailure(
    const char* node) {
  if (ImportedHostByName(node) != (struct hostent*)0) return -1;
  return ImportedHerrno() == (int*)0 ? -2 : *ImportedHerrno();
}

__attribute__((visibility("default"))) uintptr_t DnsFixtureHostStorage(
    const char* node) {
  return (uintptr_t)ImportedHostByName(node);
}

__attribute__((visibility("default"))) int DnsFixtureServiceLookup(
    const char* name, const char* proto) {
  const struct servent* service = ImportedServiceByName(name, proto);
  return service != (const struct servent*)0 && service->s_name != (char*)0 &&
                 service->s_proto != (char*)0 && service->s_port != 0
             ? 42
             : -1;
}

__attribute__((visibility("default"))) int DnsFixtureServiceFailure(
    const char* name, const char* proto) {
  if (ImportedServiceByName(name, proto) != (struct servent*)0) return -1;
  return ImportedHerrno() == (int*)0 ? -2 : *ImportedHerrno();
}

__attribute__((visibility("default"))) uintptr_t DnsFixtureServiceStorage(
    const char* name, const char* proto) {
  return (uintptr_t)ImportedServiceByName(name, proto);
}

__attribute__((visibility("default"))) int DnsFixtureLookup(
    const char* node, const char* service, int family, int flags,
    struct addrinfo** result) {
  struct addrinfo hints = {0};
  hints.ai_flags = flags;
  hints.ai_family = family;
  hints.ai_socktype = SOCK_STREAM;
  hints.ai_protocol = IPPROTO_TCP;
  return getaddrinfo(node, service, &hints, result);
}

__attribute__((visibility("default"))) int DnsFixtureLookupPassive(
    const char* service, int family, struct addrinfo** result) {
  struct addrinfo hints = {0};
  hints.ai_flags = AI_PASSIVE | AI_NUMERICSERV;
  hints.ai_family = family;
  hints.ai_socktype = SOCK_DGRAM;
  hints.ai_protocol = IPPROTO_UDP;
  return getaddrinfo((const char*)0, service, &hints, result);
}

__attribute__((visibility("default"))) int DnsFixtureCount(
    const struct addrinfo* result, int expected_family) {
  int count = 0;
  for (const struct addrinfo* current = result; current != (const struct addrinfo*)0;
       current = current->ai_next) {
    if (current->ai_family != AF_INET && current->ai_family != AF_INET6) return -10;
    if (expected_family != AF_UNSPEC && current->ai_family != expected_family) return -11;
    if (current->ai_addr == (struct sockaddr*)0 ||
        current->ai_addr->sa_family != current->ai_family) return -12;
    if (current->ai_socktype != SOCK_STREAM && current->ai_socktype != SOCK_DGRAM) return -13;
    ++count;
  }
  return count;
}

__attribute__((visibility("default"))) int DnsFixtureReverseNumeric(
    const struct addrinfo* result, char* host, size_t host_length,
    char* service, size_t service_length) {
  if (result == (const struct addrinfo*)0) return EAI_FAIL;
  return getnameinfo(result->ai_addr, result->ai_addrlen, host, host_length,
                     service, service_length,
                     NI_NUMERICHOST | NI_NUMERICSERV);
}

__attribute__((visibility("default"))) int DnsFixtureReversePolicyRejected(
    const struct addrinfo* result, char* host, size_t host_length) {
  if (result == (const struct addrinfo*)0) return EAI_FAIL;
  return getnameinfo(result->ai_addr, result->ai_addrlen, host, host_length,
                     (char*)0, 0, 0);
}

__attribute__((visibility("default"))) void DnsFixtureFree(
    struct addrinfo* result) {
  freeaddrinfo(result);
}

__attribute__((visibility("default"))) const char* DnsFixtureErrorString(
    int error) {
  return gai_strerror(error);
}

__attribute__((visibility("default"))) const char* DnsFixtureNtop(
    int family, const void* address, char* output, socklen_t output_length) {
  return inet_ntop(family, address, output, output_length);
}

__attribute__((visibility("default"))) int DnsFixtureHerrno(void) {
  int* first = ImportedHerrno();
  if (first == (int*)0) return 1;
  *first = 97;
  int* second = ImportedHerrno();
  return second == first && *second == 97 ? 42 : 2;
}
