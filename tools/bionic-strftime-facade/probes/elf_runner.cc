#include "aim_bionic_errno.h"
#include "aim_bionic_strftime.h"
#include "aim_elf_loader.h"

#include <errno.h>

#include <atomic>
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
    std::fprintf(stderr, "bionic-strftime-facade: FAIL %s\n", message);
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
  const auto function = aim_bionic_strftime_resolve(
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
  if (aim_elf_lookup(image, name, &address, &error) !=
          AIM_ELF_OK ||
      address == 0) {
    std::fprintf(stderr, "lookup %s: %s\n", name, message);
    std::abort();
  }
  return reinterpret_cast<Function>(address);
}

AimAndroidTm Sample() {
  AimAndroidTm value{};
  value.tm_sec = 7;
  value.tm_min = 5;
  value.tm_hour = 23;
  value.tm_mday = 29;
  value.tm_mon = 1;
  value.tm_year = 124;
  value.tm_wday = 4;
  value.tm_yday = 59;
  value.tm_isdst = 0;
  value.tm_gmtoff = 19800;
  value.tm_zone = "IST";
  return value;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  if (bytes.empty() || input.bad()) return 11;

  errno = EDOM;
  Check(aim_bionic_strftime_resolve("libc.so", "strftime_l",
                                            "LIBC") != nullptr &&
            errno == EDOM,
        "exact resolver and host errno");
  Check(aim_bionic_strftime_resolve("libSystem.B.dylib", "strftime_l",
                                            "LIBC") == nullptr &&
            aim_bionic_strftime_resolve("libc.so", "strftime_l",
                                                nullptr) == nullptr &&
            aim_bionic_strftime_resolve("libc.so", "strftime",
                                                "LIBC") != nullptr &&
            aim_bionic_strftime_resolve("libc.so", "strptime",
                                                "LIBC") != nullptr &&
            aim_bionic_strftime_resolve("libc.so", "tzset", "LIBC") !=
                nullptr &&
            aim_bionic_strftime_resolve("libc.so", "unknown",
                                                "LIBC") == nullptr,
        "closed resolver");
  Check(aim_bionic_strftime_activate(nullptr, 0, "DST", 0) ==
                AIM_BIONIC_STRFTIME_INVALID_ARGUMENT &&
            aim_bionic_strftime_activate("A/B", 0, "DST", 0) ==
                AIM_BIONIC_STRFTIME_INVALID_ARGUMENT &&
            aim_bionic_strftime_activate("UTC", 86401, "DST", 0) ==
                AIM_BIONIC_STRFTIME_INVALID_ARGUMENT,
        "activation validation");
  Check(aim_bionic_strftime_activate("KST", 32400, "KDT", 36000) ==
                AIM_BIONIC_STRFTIME_OK &&
            errno == EDOM,
        "activate fixed-offset owner");
  Check(aim_bionic_strftime_activate("UTC", 0, "UTC", 0) ==
            AIM_BIONIC_STRFTIME_ALREADY_ACTIVE,
        "duplicate activation rejected");

  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  AimElfHandle* image = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                  &error) == AIM_ELF_OK,
        "load Android AArch64 ELF");
  Check(aim_elf_run_initializers(image, &error) == AIM_ELF_OK,
        "initialize Android ELF");

  using Format = size_t (*)(char*, size_t, const char*,
                            const AimAndroidTm*);
  Format format = Lookup<Format>(image, "StrftimeFixtureFormat");
  AimAndroidTm value = Sample();
  char output[1024]{};

  auto Expect = [&](const char* pattern, const char* expected,
                    const char* label) {
    std::memset(output, 0xa5, sizeof(output));
    aim_bionic_errno_store(71);
    errno = EDOM;
    const size_t length = format(output, sizeof(output), pattern, &value);
    Check(length == std::strlen(expected) && std::strcmp(output, expected) == 0 &&
              aim_bionic_errno_load() == 71 && errno == EDOM,
          label);
  };

  Expect("%A|%a|%B|%b|%h|%P|%p",
         "Thursday|Thu|February|Feb|Feb|pm|PM", "C locale names");
  Expect("%C|%c|%D|%d|%e|%F|%H|%I|%j|%k|%l|%M|%m|%R|%r|%S|%T",
         "20|Thu Feb 29 23:05:07 2024|02/29/24|29|29|2024-02-29|23|11|"
         "060|23|11|05|02|23:05|11:05:07 PM|07|23:05:07",
         "numeric and composite formats");
  Expect("%U|%u|%V|%G|%g|%v|%W|%w|%X|%x|%y|%Y",
         "08|4|09|2024|24|29-Feb-2024|09|4|23:05:07|02/29/24|24|2024",
         "week and date formats");
  Expect("%Z|%z|%+", "IST|+0530|Thu Feb 29 23:05:07 IST 2024",
         "tm_zone and tm_gmtoff");
  value.tm_zone = nullptr;
  Expect("%Z|%z|%s", "KST|+0530|1709215507",
         "timezone fallback and fixed-offset epoch");
  value = Sample();
  value.tm_mday = 3;
  value.tm_hour = 5;
  value.tm_wday = 6;
  value.tm_yday = 33;
  Expect("%_d|%-d|%0e|%^B|%#p|%Od|%Ec|%Q|%%|%n|%t",
         " 3|3|03|FEBRUARY|am|03|Sat Feb  3 05:05:07 2024|Q|%|\n|\t",
         "Android modifiers and pass-through");

  value = Sample();
  char tiny[4]{};
  aim_bionic_errno_store(0);
  errno = EDOM;
  Check(format(tiny, sizeof(tiny), "%Y", &value) == 0 &&
            aim_bionic_errno_load() == 34 && errno == EDOM,
        "overflow uses Bionic ERANGE");
  aim_bionic_errno_store(0);
  Check(format(nullptr, 0, "%Y", &value) == 0 &&
            aim_bionic_errno_load() == 14,
        "null destination EFAULT");
  value.tm_isdst = -1;
  aim_bionic_errno_store(0);
  Check(format(output, sizeof(output), "%s", &value) == 0 &&
            aim_bionic_errno_load() == 95,
        "ambiguous DST inference fails closed");

  std::atomic<bool> threads_ok{true};
  std::vector<std::thread> threads;
  for (int thread_index = 0; thread_index < 8; ++thread_index) {
    threads.emplace_back([&, thread_index] {
      AimAndroidTm local = Sample();
      for (int iteration = 0; iteration < 2000; ++iteration) {
        char local_output[128]{};
        if (format(local_output, sizeof(local_output), "%F %T %Z %z", &local) !=
                29 ||
            std::strcmp(local_output, "2024-02-29 23:05:07 IST +0530") != 0) {
          threads_ok.store(false, std::memory_order_relaxed);
          return;
        }
      }
      aim_bionic_errno_store(thread_index);
      if (aim_bionic_errno_load() != thread_index) {
        threads_ok.store(false, std::memory_order_relaxed);
      }
    });
  }
  for (auto& thread : threads) thread.join();
  Check(threads_ok.load(std::memory_order_relaxed),
        "concurrent formatting and errno TLS");

  Check(aim_bionic_strftime_deactivate() ==
                AIM_BIONIC_STRFTIME_OK &&
            errno == EDOM,
        "quiescent deactivate");
  aim_bionic_errno_store(0);
  Check(format(output, sizeof(output), "%Y", &value) == 0 &&
            aim_bionic_errno_load() == 38,
        "post-deactivate ENOSYS");
  Check(aim_bionic_strftime_deactivate() ==
            AIM_BIONIC_STRFTIME_NOT_ACTIVE,
        "duplicate deactivate rejected");
  Check(aim_elf_unload(&image, &error) == AIM_ELF_OK &&
            image == nullptr,
        "unload Android ELF");
  std::fprintf(stderr,
               "bionic-strftime-facade: PASS Android-ELF=yes callsites=17 "
               "locale=C timezone=fixed-offset threads=8 ASan+UBSan\n");
  return 0;
}
