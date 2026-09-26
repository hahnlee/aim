#include "darwin_art_bionic_builtin_adapters.h"

#include <array>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

namespace {

using SymbolFunction = void (*)(void);

struct Expected {
  const char *soname;
  const char *symbol;
  const char *version;
  DarwinArtBionicProviderId owner;
  bool default_version;
};

constexpr Expected kExpected[] = {
#include "ownership.inc"
};

std::array<size_t, DARWIN_ART_BIONIC_PROVIDER_COUNT> calls{};

void Stub() {}

SymbolFunction OneArg(DarwinArtBionicProviderId owner, const char *symbol) {
  if (symbol == nullptr || symbol[0] == '\0')
    std::abort();
  ++calls[static_cast<size_t>(owner)];
  return Stub;
}

void *Triple(DarwinArtBionicProviderId owner, const char *expected_soname,
             const char *soname, const char *symbol, const char *version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, expected_soname) != 0 ||
      std::strcmp(version, "LIBC") != 0) {
    std::fprintf(stderr,
                 "bionic-provider-builtin-adapters: invalid triple expected=%s "
                 "actual=%s symbol=%s version=%s\n",
                 expected_soname, soname == nullptr ? "<null>" : soname,
                 symbol == nullptr ? "<null>" : symbol,
                 version == nullptr ? "<null>" : version);
    std::abort();
  }
  ++calls[static_cast<size_t>(owner)];
  return reinterpret_cast<void *>(&Stub);
}

} // namespace

extern "C" SymbolFunction darwin_art_bionic_libc_leaf_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_LEAF, s);
}
extern "C" SymbolFunction darwin_art_bionic_allocator_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_ALLOCATOR, s);
}
extern "C" SymbolFunction darwin_art_bionic_errno_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_ERRNO, s);
}
extern "C" SymbolFunction darwin_art_bionic_fs_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_FILESYSTEM, s);
}
extern "C" SymbolFunction darwin_art_bionic_ftw_resolve(const char *s) {
  if (std::strcmp(s, "ftw") == 0 || std::strcmp(s, "nftw") == 0)
    return OneArg(DARWIN_ART_BIONIC_PROVIDER_FILESYSTEM, s);
  return nullptr;
}
extern "C" SymbolFunction darwin_art_bionic_time_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_TIME, s);
}
extern "C" uintptr_t darwin_art_bionic_time_data_resolve(const char *s) {
  if (s != nullptr &&
      (std::strcmp(s, "daylight") == 0 || std::strcmp(s, "timezone") == 0 ||
       std::strcmp(s, "tzname") == 0)) {
    ++calls[DARWIN_ART_BIONIC_PROVIDER_TIME];
    return reinterpret_cast<uintptr_t>(&calls);
  }
  return 0;
}
extern "C" void *darwin_art_bionic_pthread_resolve(const char *soname,
                                                   const char *symbol,
                                                   const char *version) {
  if (soname != nullptr && symbol != nullptr && version != nullptr &&
      std::strcmp(soname, "libc.so") == 0 &&
      std::strcmp(symbol, "pthread_mutexattr_setprotocol") == 0 &&
      std::strcmp(version, "LIBC_P") == 0) {
    ++calls[DARWIN_ART_BIONIC_PROVIDER_PTHREAD];
    return reinterpret_cast<void *>(&Stub);
  }
  return Triple(DARWIN_ART_BIONIC_PROVIDER_PTHREAD, "libc.so", soname, symbol,
                version);
}
extern "C" SymbolFunction
darwin_art_bionic_process_state_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_PROCESS_STATE, s);
}
extern "C" int darwin_art_bionic_property_service_set(const char*, const char*) {
  return 0;
}
extern "C" uintptr_t
darwin_art_bionic_process_state_data_resolve(const char *s) {
  if (s != nullptr &&
      (std::strcmp(s, "environ") == 0 || std::strcmp(s, "optarg") == 0 ||
       std::strcmp(s, "optind") == 0)) {
    ++calls[DARWIN_ART_BIONIC_PROVIDER_PROCESS_STATE];
    return reinterpret_cast<uintptr_t>(&calls);
  }
  return 0;
}
extern "C" void *darwin_art_dl_phdr_resolve(const char *soname,
                                            const char *symbol,
                                            const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_PHDR, "libdl.so", soname, symbol,
                version);
}
extern "C" SymbolFunction darwin_art_bionic_stdio_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_STDIO, s);
}
extern "C" void *darwin_art_bionic_wide_stdio_resolve(const char *soname,
                                                      const char *symbol,
                                                      const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_WIDE_STDIO, "libc.so", soname,
                symbol, version);
}
extern "C" void *darwin_art_bionic_scanf_resolve(const char *soname,
                                                 const char *symbol,
                                                 const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_SCANF, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_bionic_swprintf_resolve(const char *soname,
                                                    const char *symbol,
                                                    const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_SWPRINTF, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_bionic_ioctl_resolve(const char *soname,
                                                 const char *symbol,
                                                 const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_IOCTL, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_bionic_strftime_resolve(const char *soname,
                                                    const char *symbol,
                                                    const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_STRFTIME, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_bionic_sendfile_resolve(const char *soname,
                                                    const char *symbol,
                                                    const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_SENDFILE, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_bionic_socket_broker_resolve(const char *soname,
                                                         const char *symbol,
                                                         const char *version) {
  const bool central =
      std::strcmp(symbol, "close") == 0 || std::strcmp(symbol, "dup") == 0 ||
      std::strcmp(symbol, "dup2") == 0 || std::strcmp(symbol, "eventfd") == 0 ||
      std::strcmp(symbol, "epoll_create") == 0 ||
      std::strcmp(symbol, "epoll_create1") == 0 ||
      std::strcmp(symbol, "epoll_ctl") == 0 ||
      std::strcmp(symbol, "epoll_wait") == 0 ||
      std::strcmp(symbol, "fcntl") == 0 || std::strcmp(symbol, "pipe") == 0 ||
      std::strcmp(symbol, "pipe2") == 0 || std::strcmp(symbol, "poll") == 0 ||
      std::strcmp(symbol, "read") == 0 ||
      std::strcmp(symbol, "__read_chk") == 0 ||
      std::strcmp(symbol, "readv") == 0 ||
      std::strcmp(symbol, "select") == 0 ||
      std::strcmp(symbol, "timerfd_create") == 0 ||
      std::strcmp(symbol, "timerfd_settime") == 0 ||
      std::strcmp(symbol, "write") == 0 ||
      std::strcmp(symbol, "__write_chk") == 0 ||
      std::strcmp(symbol, "writev") == 0;
  const auto owner = central ? DARWIN_ART_BIONIC_PROVIDER_CENTRAL_FD_BROKER
                             : DARWIN_ART_BIONIC_PROVIDER_SOCKET;
  if (central && version != nullptr &&
      std::strcmp(symbol, "__write_chk") == 0 &&
      std::strcmp(version, "LIBC_N") == 0) {
    ++calls[owner];
    return reinterpret_cast<void *>(&Stub);
  }
  if (!central && version != nullptr &&
      ((std::strcmp(version, "LIBC_N") == 0 &&
        (std::strcmp(symbol, "freeifaddrs") == 0 ||
         std::strcmp(symbol, "getifaddrs") == 0)) ||
       (std::strcmp(version, "LIBC_O") == 0 &&
        std::strcmp(symbol, "__sendto_chk") == 0) ||
       (std::strcmp(version, "LIBC_Q") == 0 &&
        (std::strcmp(symbol, "android_fdsan_close_with_tag") == 0 ||
         std::strcmp(symbol, "android_fdsan_create_owner_tag") == 0 ||
         std::strcmp(symbol, "android_fdsan_get_error_level") == 0 ||
         std::strcmp(symbol, "android_fdsan_get_owner_tag") == 0 ||
         std::strcmp(symbol, "android_fdsan_set_error_level") == 0 ||
         std::strcmp(symbol, "android_fdsan_exchange_owner_tag") == 0)))) {
    ++calls[owner];
    return reinterpret_cast<void *>(&Stub);
  }
  return Triple(owner, "libc.so", soname, symbol, version);
}
extern "C" uintptr_t darwin_art_bionic_socket_broker_data_resolve(
    const char *soname, const char *symbol, const char *version) {
  if (soname != nullptr && symbol != nullptr && version != nullptr &&
      std::strcmp(soname, "libc.so") == 0 &&
      std::strcmp(version, "LIBC_N") == 0 &&
      (std::strcmp(symbol, "in6addr_any") == 0 ||
       std::strcmp(symbol, "in6addr_loopback") == 0)) {
    ++calls[DARWIN_ART_BIONIC_PROVIDER_SOCKET];
    return reinterpret_cast<uintptr_t>(&calls);
  }
  return 0;
}
extern "C" void *darwin_art_bionic_socket_broker_dns_resolve(
    const char *soname, const char *symbol, const char *version) {
  if (soname != nullptr && symbol != nullptr && version == nullptr &&
      std::strcmp(soname, "libandroid.so") == 0) {
    ++calls[DARWIN_ART_BIONIC_PROVIDER_DNS];
    return reinterpret_cast<void *>(&Stub);
  }
  return Triple(DARWIN_ART_BIONIC_PROVIDER_DNS, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_bionic_locale_resolve(const char *soname,
                                                  const char *symbol,
                                                  const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_LOCALE, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_bionic_math_resolve(const char *soname,
                                                const char *symbol,
                                                const char *version) {
  if (std::strcmp(soname, "libc.so") == 0 &&
      (std::strcmp(symbol, "ldexp") == 0 ||
       std::strcmp(symbol, "feclearexcept") == 0 ||
       std::strcmp(symbol, "feraiseexcept") == 0)) {
    if (std::strcmp(version, "LIBC") != 0)
      std::abort();
    ++calls[DARWIN_ART_BIONIC_PROVIDER_MATH];
    return reinterpret_cast<void *>(&Stub);
  }
  return Triple(DARWIN_ART_BIONIC_PROVIDER_MATH, "libm.so", soname, symbol,
                version);
}
extern "C" SymbolFunction darwin_art_bionic_vm_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_VM, s);
}
extern "C" SymbolFunction darwin_art_bionic_numeric_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_NUMERIC, s);
}
extern "C" void *darwin_art_bionic_float_conversion_resolve(
    const char *soname, const char *symbol, const char *version) {
  const char *expected_version = std::strcmp(symbol, "strtod_l") == 0 ||
                                         std::strcmp(symbol, "strtof_l") == 0
                                     ? "LIBC_O"
                                     : "LIBC";
  if (soname == nullptr || version == nullptr ||
      std::strcmp(soname, "libc.so") != 0 ||
      std::strcmp(version, expected_version) != 0)
    std::abort();
  ++calls[DARWIN_ART_BIONIC_PROVIDER_FLOAT_CONVERSION];
  return reinterpret_cast<void *>(&Stub);
}
extern "C" SymbolFunction darwin_art_bionic_format_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_FORMAT, s);
}
extern "C" SymbolFunction darwin_art_bionic_strerror_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_STRERROR, s);
}
extern "C" void *darwin_art_bionic_wide_integer_resolve(const char *soname,
                                                        const char *symbol,
                                                        const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_WIDE_INTEGER, "libc.so", soname,
                symbol, version);
}
extern "C" void *darwin_art_bionic_wide_float_resolve(const char *soname,
                                                      const char *symbol,
                                                      const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_WIDE_FLOAT, "libc.so", soname,
                symbol, version);
}
extern "C" void *darwin_art_bionic_binary128_conversion_resolve(
    const char *soname, const char *symbol, const char *version) {
  if (soname != nullptr && symbol != nullptr && version != nullptr &&
      std::strcmp(soname, "libm.so") == 0 &&
      std::strcmp(symbol, "powl") == 0 &&
      std::strcmp(version, "LIBC") == 0) {
    ++calls[DARWIN_ART_BIONIC_PROVIDER_BINARY128_CONVERSION];
    return reinterpret_cast<void *>(&Stub);
  }
  return Triple(DARWIN_ART_BIONIC_PROVIDER_BINARY128_CONVERSION, "libc.so",
                soname, symbol, version);
}
extern "C" void *darwin_art_bionic_abort_resolve(const char *soname,
                                                 const char *symbol,
                                                 const char *version) {
  return Triple(DARWIN_ART_BIONIC_PROVIDER_ABORT, "libc.so", soname, symbol,
                version);
}
extern "C" void *darwin_art_android_binder_ndk_resolve(
    const char *soname, const char *symbol, const char *version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, "libbinder_ndk.so") != 0 ||
      std::strcmp(version, "LIBBINDER_NDK") != 0)
    std::abort();
  ++calls[DARWIN_ART_BIONIC_PROVIDER_BINDER_NDK];
  return reinterpret_cast<void *>(&Stub);
}
extern "C" void *darwin_art_android_aaudio_resolve(
    const char *soname, const char *symbol, const char *version) {
  if (soname == nullptr || symbol == nullptr || version != nullptr ||
      std::strcmp(soname, "libaaudio.so") != 0)
    std::abort();
  ++calls[DARWIN_ART_BIONIC_PROVIDER_AAUDIO];
  return reinterpret_cast<void *>(&Stub);
}
extern "C" uintptr_t darwin_art_liblog_provider_resolve(const char *symbol,
                                                        const char *version) {
  bool matches = false;
  for (const auto& expected : kExpected) {
    if (expected.owner == DARWIN_ART_BIONIC_PROVIDER_LIBLOG && symbol &&
        std::strcmp(expected.symbol, symbol) == 0 &&
        ((version && std::strcmp(expected.version, version) == 0) ||
         ((!version || !version[0]) && expected.default_version))) matches = true;
  }
  if (!matches) std::abort();
  ++calls[DARWIN_ART_BIONIC_PROVIDER_LIBLOG];
  return reinterpret_cast<uintptr_t>(&Stub);
}
extern "C" SymbolFunction
darwin_art_bionic_dso_lifecycle_resolve(const char *s) {
  return OneArg(DARWIN_ART_BIONIC_PROVIDER_DSO_LIFECYCLE, s);
}
extern "C" SymbolFunction
darwin_art_bionic_syslog_resolve(const char *soname, const char *symbol,
                                 const char *version) {
  return reinterpret_cast<SymbolFunction>(Triple(
      DARWIN_ART_BIONIC_PROVIDER_SYSLOG, "libc.so", soname, symbol, version));
}
extern "C" SymbolFunction darwin_art_bionic_formatted_stdio_resolve(
    const char *soname, const char *symbol, const char *version) {
  return reinterpret_cast<SymbolFunction>(
      Triple(DARWIN_ART_BIONIC_PROVIDER_FORMATTED_STDIO, "libc.so", soname,
             symbol, version));
}
extern "C" SymbolFunction
darwin_art_bionic_syscall_resolve(const char *soname, const char *symbol,
                                  const char *version) {
  return reinterpret_cast<SymbolFunction>(Triple(
      DARWIN_ART_BIONIC_PROVIDER_SYSCALL, "libc.so", soname, symbol, version));
}

int main() {
  DarwinArtBionicNamespace *instance = darwin_art_bionic_namespace_create();
  if (instance == nullptr ||
      darwin_art_bionic_namespace_bind_builtins(instance, nullptr) !=
          DARWIN_ART_BIONIC_NAMESPACE_OK ||
      darwin_art_bionic_namespace_seal(instance) !=
          DARWIN_ART_BIONIC_NAMESPACE_OK)
    return 10;
  for (const Expected &expected : kExpected) {
    const auto result = darwin_art_bionic_namespace_resolve(
        instance, expected.soname, expected.symbol,
        expected.version[0] == '\0' ? nullptr : expected.version);
    if (result.status != DARWIN_ART_BIONIC_NAMESPACE_OK ||
        result.owner != expected.owner || result.address == 0)
      return 11;
  }
  // Process state (index 6): __system_property_set resolves directly to the
  // property service client, without a resolver call.
  constexpr size_t kExpectedCalls[] = {
      67, 14, 1, 79, 21, 64, 108, 1, 48, 63, 11, 5, 8, 3, 4, 5,  19,
      10, 2,  4, 3,  2,  4,   3,  3,  1, 1, 4, 1, 21, 40, 13, 85, 10, 39, 30};
  for (size_t index = 0; index < calls.size(); ++index) {
    if (calls[index] != kExpectedCalls[index]) {
      std::fprintf(stderr,
                   "bionic-provider-builtin-adapters: call-count mismatch "
                   "provider=%zu expected=%zu actual=%zu\n",
                   index, kExpectedCalls[index], calls[index]);
      return 12;
    }
  }
  if (darwin_art_bionic_namespace_teardown(instance) !=
      DARWIN_ART_BIONIC_NAMESPACE_OK)
    return 13;
  darwin_art_bionic_namespace_destroy(instance);
  std::fprintf(stderr, "bionic-provider-builtin-adapters: PASS providers=36 "
                       "routes=798 version-aliases=exact\n");
  return 0;
}
