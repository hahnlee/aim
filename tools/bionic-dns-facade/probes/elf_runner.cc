#include "aim_bionic_dns.h"
#include "aim_elf_loader.h"

#include <errno.h>

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <thread>
#include <vector>

namespace {

void Check(bool condition, const char* message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-dns-facade: FAIL %s\n", message);
    std::abort();
  }
}

AimElfResolveStatus Resolve(void*,
                                  const AimElfSymbolRequest* request,
                                  uintptr_t* output,
                                  AimElfErrorBuffer*) {
  if (request == nullptr || output == nullptr) {
    return AIM_ELF_RESOLVE_ERROR;
  }
  const auto function = aim_bionic_dns_resolve(
      request->version_soname, request->symbol, request->version_name);
  if (function == nullptr) return AIM_ELF_RESOLVE_NOT_FOUND;
  *output = reinterpret_cast<uintptr_t>(function);
  return AIM_ELF_RESOLVE_FOUND;
}

template <typename Function>
Function Lookup(AimElfHandle* image, const char* name) {
  uintptr_t address = 0;
  char message[256]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_lookup(image, name, &address, &error) ==
            AIM_ELF_OK &&
            address != 0,
        name);
  return reinterpret_cast<Function>(address);
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  if (bytes.empty() || input.bad()) return 11;

  const char* symbols[] = {
      "freeaddrinfo", "gai_strerror", "getaddrinfo", "getnameinfo",
      "inet_ntop", "__get_h_errno", "gethostbyname", "getservbyname"};
  for (const char* symbol : symbols) {
    Check(aim_bionic_dns_resolve("libc.so", symbol, "LIBC") != nullptr,
          "exact resolver entry");
  }
  Check(aim_bionic_dns_resolve("libSystem.B.dylib", "getaddrinfo",
                                      "LIBC") == nullptr &&
            aim_bionic_dns_resolve("libc.so", "getaddrinfo", nullptr) ==
                nullptr,
        "closed resolver");

  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  AimElfHandle* image = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                  &error) == AIM_ELF_OK,
        "load actual Android ELF");
  Check(aim_elf_run_initializers(image, &error) == AIM_ELF_OK,
        "run initializers");

  using LookupAddress = int (*)(const char*, const char*, int, int,
                                AimAndroidAddrinfo**);
  using LookupPassive = int (*)(const char*, int,
                                AimAndroidAddrinfo**);
  using Count = int (*)(const AimAndroidAddrinfo*, int);
  using Reverse = int (*)(const AimAndroidAddrinfo*, char*, size_t,
                          char*, size_t);
  using ReverseRejected = int (*)(const AimAndroidAddrinfo*, char*,
                                  size_t);
  using Free = void (*)(AimAndroidAddrinfo*);
  using ErrorString = const char* (*)(int);
  using Ntop = const char* (*)(int, const void*, char*, uint32_t);
  using Herrno = int* (*)(void);
  using HerrnoFixture = int (*)(void);
  using LegacyLookup = int (*)(const char*);
  using LegacyFailure = int (*)(const char*);
  using LegacyStorage = uintptr_t (*)(const char*);
  using ServiceLookup = int (*)(const char*, const char*);
  using ServiceFailure = int (*)(const char*, const char*);
  using ServiceStorage = uintptr_t (*)(const char*, const char*);
  LookupAddress lookup = Lookup<LookupAddress>(image, "DnsFixtureLookup");
  LookupPassive passive = Lookup<LookupPassive>(image, "DnsFixtureLookupPassive");
  Count count = Lookup<Count>(image, "DnsFixtureCount");
  Reverse reverse = Lookup<Reverse>(image, "DnsFixtureReverseNumeric");
  ReverseRejected reverse_rejected =
      Lookup<ReverseRejected>(image, "DnsFixtureReversePolicyRejected");
  Free free_result = Lookup<Free>(image, "DnsFixtureFree");
  ErrorString error_string = Lookup<ErrorString>(image, "DnsFixtureErrorString");
  Ntop ntop = Lookup<Ntop>(image, "DnsFixtureNtop");
  HerrnoFixture h_errno_fixture = Lookup<HerrnoFixture>(image, "DnsFixtureHerrno");
  LegacyLookup host_lookup = Lookup<LegacyLookup>(image, "DnsFixtureHostLookup");
  LegacyFailure host_failure =
      Lookup<LegacyFailure>(image, "DnsFixtureHostFailure");
  LegacyStorage host_storage =
      Lookup<LegacyStorage>(image, "DnsFixtureHostStorage");
  ServiceLookup service_lookup =
      Lookup<ServiceLookup>(image, "DnsFixtureServiceLookup");
  ServiceFailure service_failure =
      Lookup<ServiceFailure>(image, "DnsFixtureServiceFailure");
  ServiceStorage service_storage =
      Lookup<ServiceStorage>(image, "DnsFixtureServiceStorage");
  Herrno get_h_errno = reinterpret_cast<Herrno>(
      aim_bionic_dns_resolve("libc.so", "__get_h_errno", "LIBC"));
  Check(get_h_errno != nullptr, "thread-local h_errno resolver entry");
  *get_h_errno() = 11;
  int* main_h_errno = get_h_errno();
  std::atomic<bool> worker_h_errno_is_distinct{false};
  std::thread h_errno_worker([&] {
    int* worker_h_errno = get_h_errno();
    worker_h_errno_is_distinct.store(worker_h_errno != main_h_errno &&
                                         *worker_h_errno == 0,
                                     std::memory_order_release);
    *worker_h_errno = 12;
  });
  h_errno_worker.join();
  Check(worker_h_errno_is_distinct.load(std::memory_order_acquire) &&
            *main_h_errno == 11,
        "h_errno is Android thread-local storage");
  Check(h_errno_fixture() == 42, "Android ELF h_errno import ABI");
  Check(host_lookup("127.0.0.1") == 42 && host_lookup("localhost") == 42,
        "legacy hostent numeric and localhost lookup");
  const int missing_host = host_failure(
      "aim-host-does-not-exist.invalid");
  Check(missing_host == HOST_NOT_FOUND || missing_host == NO_DATA,
        "legacy hostent failure updates h_errno");
  Check(service_lookup("https", "tcp") == 42,
        "legacy servent known service lookup");
  Check(service_failure("aim-service-does-not-exist", "tcp") ==
            NO_DATA,
        "legacy servent failure updates h_errno");
  const uintptr_t host_storage_main = host_storage("127.0.0.1");
  const uintptr_t service_storage_main = service_storage("https", "tcp");
  std::atomic<bool> legacy_storage_distinct{false};
  std::thread legacy_storage_worker([&] {
    legacy_storage_distinct.store(
        host_storage("127.0.0.1") != host_storage_main &&
            service_storage("https", "tcp") != service_storage_main,
        std::memory_order_release);
  });
  legacy_storage_worker.join();
  Check(legacy_storage_distinct.load(std::memory_order_acquire),
        "legacy hostent and servent storage is thread-local");
  constexpr const char* kKnownServices[] = {"https", "http", "ssh"};
  std::atomic<bool> service_stress_ok{true};
  std::vector<std::thread> service_workers;
  for (size_t worker_index = 0; worker_index < 8; ++worker_index) {
    service_workers.emplace_back([&, worker_index] {
      for (size_t iteration = 0; iteration < 64; ++iteration) {
        const char* service =
            kKnownServices[(worker_index + iteration) %
                           (sizeof(kKnownServices) / sizeof(kKnownServices[0]))];
        if (service_lookup(service, "tcp") != 42 ||
            service_storage(service, "tcp") == 0) {
          service_stress_ok.store(false, std::memory_order_release);
          return;
        }
      }
    });
  }
  for (std::thread& worker : service_workers) worker.join();
  Check(service_stress_ok.load(std::memory_order_acquire),
        "concurrent varied service lookup remains stable");

  constexpr int kAfUnspec = 0;
  constexpr int kAfInet = 2;
  constexpr int kAfInet6 = 10;
  constexpr int kAiCanonname = 0x2;
  constexpr int kAiNumericHost = 0x4;
  constexpr int kAiNumericServ = 0x8;
  errno = EDOM;

  const uint8_t loopback4[] = {127, 0, 0, 1};
  const uint8_t loopback6[] = {0, 0, 0, 0, 0, 0, 0, 0,
                               0, 0, 0, 0, 0, 0, 0, 1};
  char presentation[64]{};
  Check(ntop(kAfInet, loopback4, presentation, sizeof(presentation)) ==
                presentation &&
            std::strcmp(presentation, "127.0.0.1") == 0,
        "inet_ntop Android IPv4 family");
  std::memset(presentation, 0, sizeof(presentation));
  Check(ntop(kAfInet6, loopback6, presentation, sizeof(presentation)) ==
                presentation &&
            std::strcmp(presentation, "::1") == 0,
        "inet_ntop Android IPv6 family");

  AimAndroidAddrinfo* ipv4 = nullptr;
  Check(lookup("127.0.0.1", "443", kAfInet,
               kAiNumericHost | kAiNumericServ, &ipv4) == 0 &&
            ipv4 != nullptr && count(ipv4, kAfInet) >= 1,
        "numeric IPv4 lookup and Android addrinfo");
  char host[128]{};
  char service[32]{};
  Check(reverse(ipv4, host, sizeof(host), service, sizeof(service)) == 0 &&
            std::strcmp(host, "127.0.0.1") == 0 &&
            std::strcmp(service, "443") == 0,
        "numeric IPv4 reverse");
  Check(reverse_rejected(ipv4, host, sizeof(host)) == 8,
        "reverse name policy closed");

  AimAndroidAddrinfo* ipv6 = nullptr;
  Check(lookup("::1", "53", kAfInet6,
               kAiNumericHost | kAiNumericServ, &ipv6) == 0 &&
            ipv6 != nullptr && count(ipv6, kAfInet6) >= 1,
        "numeric IPv6 lookup and Android sockaddr");
  std::memset(host, 0, sizeof(host));
  std::memset(service, 0, sizeof(service));
  Check(reverse(ipv6, host, sizeof(host), service, sizeof(service)) == 0 &&
            std::strcmp(host, "::1") == 0 && std::strcmp(service, "53") == 0,
        "numeric IPv6 reverse");

  AimAndroidAddrinfo* localhost = nullptr;
  Check(lookup("localhost", "80", kAfUnspec,
               kAiCanonname | kAiNumericServ, &localhost) == 0 &&
            localhost != nullptr && count(localhost, kAfUnspec) >= 1,
        "bounded localhost lookup");
  AimAndroidAddrinfo* wildcard = nullptr;
  Check(passive("9000", kAfInet, &wildcard) == 0 && wildcard != nullptr &&
            count(wildcard, kAfInet) >= 1,
        "passive numeric service");

  AimAndroidAddrinfo* rejected = nullptr;
  Check(lookup("unqualified", "443", kAfInet, kAiNumericServ,
               &rejected) == 8 && rejected == nullptr &&
            lookup("printer.local", "443", kAfInet, kAiNumericServ,
                   &rejected) == 8 && rejected == nullptr &&
            lookup("printer.LOCAL.", "443", kAfInet, kAiNumericServ,
                   &rejected) == 8 && rejected == nullptr &&
            lookup("bad/name.example", "443", kAfInet, kAiNumericServ,
                   &rejected) == 8 && rejected == nullptr,
        "search mDNS and malformed name policy closed");
  Check(lookup("127.0.0.1", "https", kAfInet, kAiNumericHost,
               &rejected) == 9 && rejected == nullptr,
        "named service policy closed");
  AimAndroidAddrinfo* configured = nullptr;
  Check(lookup("127.0.0.1", "443", kAfInet,
               kAiNumericHost | kAiNumericServ | 0x400,
               &configured) == 0 && configured != nullptr,
        "AI_ADDRCONFIG translated");
  Check(std::strcmp(error_string(8), "Name or service not known") == 0 &&
            std::strcmp(error_string(999), "Unknown error") == 0,
        "Android gai_strerror table");
  Check(errno == EDOM, "host errno preserved");

  AimAndroidAddrinfo* concurrent = nullptr;
  Check(lookup("127.0.0.1", "1234", kAfInet,
               kAiNumericHost | kAiNumericServ, &concurrent) == 0,
        "concurrent result setup");
  std::atomic<bool> reader_started{false};
  std::atomic<bool> stop_reader{false};
  std::atomic<bool> reader_ok{true};
  std::thread reader([&] {
    errno = EDOM;
    reader_started.store(true, std::memory_order_release);
    while (!stop_reader.load(std::memory_order_acquire)) {
      char local_host[64]{};
      char local_service[16]{};
      if (count(concurrent, kAfInet) < 1 ||
          reverse(concurrent, local_host, sizeof(local_host), local_service,
                  sizeof(local_service)) != 0 ||
          std::strcmp(local_host, "127.0.0.1") != 0 || errno != EDOM) {
        reader_ok.store(false, std::memory_order_release);
        break;
      }
    }
  });
  while (!reader_started.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
  std::thread first_free([&] { free_result(concurrent); });
  std::thread duplicate_free([&] { free_result(concurrent); });
  first_free.join();
  duplicate_free.join();
  Check(aim_bionic_dns_retired_results_for_test() >= 1,
        "concurrent free atomically retires");
  stop_reader.store(true, std::memory_order_release);
  reader.join();
  Check(reader_ok.load(std::memory_order_acquire),
        "reader remains memory-safe after logical free");

  free_result(ipv4);
  free_result(ipv6);
  free_result(localhost);
  free_result(wildcard);
  free_result(configured);
  Check(aim_bionic_dns_live_results_for_test() == 0 &&
            aim_bionic_dns_retired_results_for_test() == 6,
        "all allocations retired");
  aim_bionic_dns_reset_for_test();
  Check(aim_bionic_dns_live_results_for_test() == 0 &&
            aim_bionic_dns_retired_results_for_test() == 0,
        "quiescent reset reclaims all results");

  Check(aim_elf_unload(&image, &error) == AIM_ELF_OK &&
            image == nullptr,
        "unload Android ELF");
  std::fprintf(stderr,
               "bionic-dns-facade: PASS Android-ELF=yes localhost+IPv4+IPv6="
               "yes reverse=numeric legacy-hostent+servent=deep-copy+TLS "
               "allocation=retire+quiescent-reset policy=absolute-host-dns "
               "errno=preserved\n");
  return 0;
}
