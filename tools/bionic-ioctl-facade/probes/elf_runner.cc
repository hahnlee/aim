#include "aim_bionic_errno.h"
#include "aim_bionic_ioctl.h"
#include "aim_elf_loader.h"

#include <errno.h>
#include <sys/mman.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <iterator>
#include <mutex>
#include <condition_variable>
#include <thread>
#include <vector>

namespace {

struct ProviderContext {
  std::mutex mutex;
  std::condition_variable condition;
  bool block = false;
  bool entered = false;
  bool release = false;
};

AimBionicIoctlFdLookupStatus DescribeFd(
    void* opaque, int32_t fd, AimBionicIoctlFdInfo* info) {
  errno = ERANGE;
  if (info == nullptr) return AIM_BIONIC_IOCTL_FD_CAPABILITY_UNAVAILABLE;
  if (fd == 10002) return AIM_BIONIC_IOCTL_FD_CAPABILITY_UNAVAILABLE;
  if (fd < 10000 || fd > 10004) return AIM_BIONIC_IOCTL_FD_BAD;
  info->abi_version = AIM_BIONIC_IOCTL_FD_INFO_ABI_VERSION;
  if (fd == 10001) {
    info->kind = AIM_BIONIC_IOCTL_FD_OTHER;
    return AIM_BIONIC_IOCTL_FD_FOUND;
  }
  info->kind = AIM_BIONIC_IOCTL_FD_RANDOM_DEVICE;
  if (fd == 10004) info->abi_version = 999;
  if (fd == 10003) {
    auto* context = static_cast<ProviderContext*>(opaque);
    std::unique_lock<std::mutex> lock(context->mutex);
    if (context->block) {
      context->entered = true;
      context->condition.notify_all();
      context->condition.wait(lock, [context] { return context->release; });
    }
  }
  return AIM_BIONIC_IOCTL_FD_FOUND;
}

AimElfResolveStatus Resolve(void*,
                                  const AimElfSymbolRequest* request,
                                  uintptr_t* output,
                                  AimElfErrorBuffer*) {
  if (request == nullptr || output == nullptr) {
    return AIM_ELF_RESOLVE_ERROR;
  }
  const auto function = aim_bionic_ioctl_resolve(
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

void Check(bool condition, const char* message) {
  if (!condition) {
    std::fprintf(stderr, "bionic-ioctl-facade: FAIL %s\n", message);
    std::abort();
  }
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 2) return 10;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  if (bytes.empty() || input.bad()) return 11;

  Check(aim_bionic_ioctl_resolve("libc.so", "ioctl", "LIBC") !=
            nullptr,
        "exact resolver");
  Check(aim_bionic_ioctl_resolve("libSystem.B.dylib", "ioctl",
                                        "LIBC") == nullptr &&
            aim_bionic_ioctl_resolve("libc.so", "ioctl", nullptr) ==
                nullptr &&
            aim_bionic_ioctl_resolve("libc.so", "unknown", "LIBC") ==
                nullptr,
        "closed resolver");
  Check(aim_bionic_ioctl_activate(nullptr, nullptr) ==
            AIM_BIONIC_IOCTL_LIFECYCLE_INVALID_ARGUMENT,
        "reject null provider");

  ProviderContext context;
  errno = EDOM;
  Check(aim_bionic_ioctl_activate(DescribeFd, &context) ==
            AIM_BIONIC_IOCTL_LIFECYCLE_OK &&
            errno == EDOM,
        "activate and preserve host errno");
  Check(aim_bionic_ioctl_activate(DescribeFd, &context) ==
            AIM_BIONIC_IOCTL_LIFECYCLE_ALREADY_ACTIVE,
        "single active fd owner");

  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve, nullptr};
  AimElfHandle* image = nullptr;
  char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  Check(aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image,
                                  &error) == AIM_ELF_OK,
        "load Android ELF");
  Check(aim_elf_run_initializers(image, &error) == AIM_ELF_OK,
        "run initializers");

  using Entropy = int (*)(int, int32_t*);
  using Null = int (*)(int);
  Entropy entropy = Lookup<Entropy>(image, "IoctlFixtureEntropy");
  Entropy unknown = Lookup<Entropy>(image, "IoctlFixtureUnknown");
  Null null_argument = Lookup<Null>(image, "IoctlFixtureNull");

  int32_t bits = -1;
  aim_bionic_errno_store(77);
  errno = EDOM;
  Check(entropy(10000, &bits) == 0 && bits == 32 &&
            aim_bionic_errno_load() == 77 && errno == EDOM,
        "entropy count and errno preservation");

  bits = 123;
  aim_bionic_errno_store(0);
  Check(entropy(9999, &bits) == -1 && bits == 123 &&
            aim_bionic_errno_load() == 9,
        "unknown fd EBADF");
  aim_bionic_errno_store(0);
  Check(entropy(10001, &bits) == -1 &&
            aim_bionic_errno_load() == 25,
        "known non-entropy fd ENOTTY");
  aim_bionic_errno_store(0);
  Check(entropy(10002, &bits) == -1 &&
            aim_bionic_errno_load() == 38,
        "fd capability ENOSYS");
  aim_bionic_errno_store(0);
  Check(entropy(10004, &bits) == -1 &&
            aim_bionic_errno_load() == 5,
        "invalid provider metadata EIO");
  aim_bionic_errno_store(0);
  Check(unknown(10000, &bits) == -1 &&
            aim_bionic_errno_load() == 25,
        "unknown request ENOTTY");
  aim_bionic_errno_store(0);
  Check(null_argument(10000) == -1 &&
            aim_bionic_errno_load() == 14,
        "null output EFAULT");
  alignas(4) uint8_t unaligned_storage[8]{};
  aim_bionic_errno_store(0);
  Check(entropy(10000,
                reinterpret_cast<int32_t*>(unaligned_storage + 1)) == -1 &&
            aim_bionic_errno_load() == 14,
        "unaligned output EFAULT");

  const long page = sysconf(_SC_PAGESIZE);
  Check(page > 0, "page size");
  void* mapping = mmap(nullptr, static_cast<size_t>(page) * 2,
                       PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
  Check(mapping != MAP_FAILED, "guard mapping");
  auto* guard = reinterpret_cast<int32_t*>(
      static_cast<uint8_t*>(mapping) + page);
  Check(mprotect(guard, static_cast<size_t>(page), PROT_NONE) == 0,
        "guard protection");
  aim_bionic_errno_store(0);
  Check(entropy(10000, guard) == -1 &&
            aim_bionic_errno_load() == 14,
        "guard output EFAULT");
  Check(mprotect(mapping, static_cast<size_t>(page), PROT_READ) == 0,
        "read-only protection");
  aim_bionic_errno_store(0);
  Check(entropy(10000, static_cast<int32_t*>(mapping)) == -1 &&
            aim_bionic_errno_load() == 14,
        "read-only output EFAULT");
  Check(munmap(mapping, static_cast<size_t>(page) * 2) == 0,
        "unmap guard");
  aim_bionic_errno_store(0);
  Check(entropy(10000, guard) == -1 &&
            aim_bionic_errno_load() == 14,
        "unmapped output EFAULT");
  Check(errno == EDOM, "failed calls preserve host errno");

  {
    std::lock_guard<std::mutex> lock(context.mutex);
    context.block = true;
  }
  std::atomic<bool> guest_done{false};
  std::atomic<bool> deactivate_started{false};
  std::atomic<bool> deactivate_done{false};
  std::thread guest([&] {
    int32_t concurrent_bits = -1;
    Check(entropy(10003, &concurrent_bits) == 0 && concurrent_bits == 32,
          "in-flight entropy");
    guest_done.store(true, std::memory_order_release);
  });
  {
    std::unique_lock<std::mutex> lock(context.mutex);
    context.condition.wait(lock, [&context] { return context.entered; });
  }
  std::thread teardown([&] {
    deactivate_started.store(true, std::memory_order_release);
    Check(aim_bionic_ioctl_deactivate() ==
              AIM_BIONIC_IOCTL_LIFECYCLE_OK,
          "deactivate");
    deactivate_done.store(true, std::memory_order_release);
  });
  while (!deactivate_started.load(std::memory_order_acquire)) {
    std::this_thread::yield();
  }
  std::this_thread::sleep_for(std::chrono::milliseconds(20));
  Check(!deactivate_done.load(std::memory_order_acquire) &&
            !guest_done.load(std::memory_order_acquire),
        "deactivate waits for fd callback");
  {
    std::lock_guard<std::mutex> lock(context.mutex);
    context.release = true;
    context.condition.notify_all();
  }
  guest.join();
  teardown.join();
  aim_bionic_errno_store(0);
  Check(entropy(10000, &bits) == -1 &&
            aim_bionic_errno_load() == 38,
        "post-deactivate admission denied");
  Check(aim_bionic_ioctl_deactivate() ==
            AIM_BIONIC_IOCTL_LIFECYCLE_OK,
        "idempotent deactivate");

  Check(aim_elf_unload(&image, &error) == AIM_ELF_OK &&
            image == nullptr,
        "unload Android ELF");
  std::fprintf(stderr,
               "bionic-ioctl-facade: PASS Android-ELF=yes request="
               "RNDGETENTCNT fd-seam=kind+quiescent errno=Bionic "
               "host-ioctl=0 ASan+UBSan\n");
  return 0;
}
