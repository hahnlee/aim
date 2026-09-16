#include "darwin_art_bionic_ioctl.h"

#include <errno.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>

#include <atomic>

extern "C" void darwin_art_bionic_errno_store(int32_t android_errno);
extern "C" int darwin_art_bionic_fd_broker_ioctl_dispatch(
    int fd, uint32_t request, void* argument, int* handled, int* result,
    int* android_errno) __attribute__((weak_import));

using SharedMemoryIoctl = int (*)(int, uint32_t, void*, int*, int*);
std::atomic<SharedMemoryIoctl> g_shared_memory_ioctl{nullptr};

namespace {

constexpr uint32_t kRndGetEntCnt = 0x80045200U;
constexpr int32_t kRandomDeviceEntropyBits = 32;
constexpr int32_t kAndroidEbadf = 9;
constexpr int32_t kAndroidEfault = 14;
constexpr int32_t kAndroidEio = 5;
constexpr int32_t kAndroidEnotty = 25;
constexpr int32_t kAndroidEnosys = 38;

struct ProviderState {
  DarwinArtBionicIoctlFdLookup lookup = nullptr;
  void* context = nullptr;
  size_t in_flight = 0;
  bool draining = false;
};

pthread_mutex_t g_provider_lock = PTHREAD_MUTEX_INITIALIZER;
pthread_cond_t g_provider_condition = PTHREAD_COND_INITIALIZER;
ProviderState g_provider;

bool IsWritableRange(void* pointer, size_t length) {
  const uintptr_t begin = reinterpret_cast<uintptr_t>(pointer);
  if (begin == 0 || length == 0 || begin > UINTPTR_MAX - length) return false;
  const uintptr_t end = begin + length;
  mach_vm_address_t region = static_cast<mach_vm_address_t>(begin);
  mach_vm_size_t region_size = 0;
  vm_region_basic_info_data_64_t info{};
  mach_msg_type_number_t count = VM_REGION_BASIC_INFO_COUNT_64;
  mach_port_t object = MACH_PORT_NULL;
  const kern_return_t status = mach_vm_region(
      mach_task_self(), &region, &region_size, VM_REGION_BASIC_INFO_64,
      reinterpret_cast<vm_region_info_t>(&info), &count, &object);
  if (object != MACH_PORT_NULL) {
    (void)mach_port_deallocate(mach_task_self(), object);
  }
  constexpr vm_prot_t kRequired = VM_PROT_READ | VM_PROT_WRITE;
  if (status != KERN_SUCCESS || (info.protection & kRequired) != kRequired ||
      region > begin || region_size > UINT64_MAX - region) {
    return false;
  }
  return end <= region + region_size;
}

bool WriteInt32(void* pointer, int32_t value) {
  const uintptr_t address = reinterpret_cast<uintptr_t>(pointer);
  if ((address & (alignof(int32_t) - 1)) != 0 ||
      !IsWritableRange(pointer, sizeof(value))) {
    return false;
  }
  return mach_vm_write(mach_task_self(), static_cast<mach_vm_address_t>(address),
                       reinterpret_cast<vm_offset_t>(&value), sizeof(value)) ==
         KERN_SUCCESS;
}

int Fail(int32_t android_errno) {
  darwin_art_bionic_errno_store(android_errno);
  return -1;
}

bool AcquireProvider(DarwinArtBionicIoctlFdLookup* lookup, void** context) {
  (void)pthread_mutex_lock(&g_provider_lock);
  if (g_provider.lookup == nullptr || g_provider.draining) {
    (void)pthread_mutex_unlock(&g_provider_lock);
    return false;
  }
  ++g_provider.in_flight;
  *lookup = g_provider.lookup;
  *context = g_provider.context;
  (void)pthread_mutex_unlock(&g_provider_lock);
  return true;
}

void ReleaseProvider() {
  (void)pthread_mutex_lock(&g_provider_lock);
  if (g_provider.in_flight == 0) __builtin_trap();
  --g_provider.in_flight;
  if (g_provider.draining && g_provider.in_flight == 0) {
    (void)pthread_cond_broadcast(&g_provider_condition);
  }
  (void)pthread_mutex_unlock(&g_provider_lock);
}

int Dispatch(int32_t fd, uint32_t request, void* argument) {
  int shared_result = -1;
  int shared_errno = 0;
  const SharedMemoryIoctl shared_memory_ioctl =
      g_shared_memory_ioctl.load(std::memory_order_acquire);
  if (shared_memory_ioctl != nullptr &&
      shared_memory_ioctl(fd, request, argument, &shared_result,
                          &shared_errno) != 0) {
    return shared_result < 0
               ? Fail(shared_errno == 0 ? kAndroidEio : shared_errno)
               : shared_result;
  }
  // Socket, pipe, eventfd, timerfd, and SCM-imported descriptors live in the
  // central fd broker rather than the filesystem table. Give that owner the
  // first chance to implement Linux device ioctls (notably sync_file fences)
  // before classifying the descriptor through the filesystem provider.
  if (darwin_art_bionic_fd_broker_ioctl_dispatch != nullptr) {
    int handled = 0;
    int broker_result = -1;
    int broker_errno = 0;
    if (darwin_art_bionic_fd_broker_ioctl_dispatch(
            fd, request, argument, &handled, &broker_result,
            &broker_errno) != 0) {
      return Fail(kAndroidEio);
    }
    if (handled != 0) {
      return broker_result < 0
                 ? Fail(broker_errno == 0 ? kAndroidEio : broker_errno)
                 : broker_result;
    }
  }
  DarwinArtBionicIoctlFdLookup lookup = nullptr;
  void* context = nullptr;
  if (!AcquireProvider(&lookup, &context)) return Fail(kAndroidEnosys);

  DarwinArtBionicIoctlFdInfo info{
      DARWIN_ART_BIONIC_IOCTL_FD_INFO_ABI_VERSION,
      DARWIN_ART_BIONIC_IOCTL_FD_OTHER,
  };
  const DarwinArtBionicIoctlFdLookupStatus status = lookup(context, fd, &info);
  ReleaseProvider();

  if (status == DARWIN_ART_BIONIC_IOCTL_FD_BAD) return Fail(kAndroidEbadf);
  if (status == DARWIN_ART_BIONIC_IOCTL_FD_CAPABILITY_UNAVAILABLE) {
    return Fail(kAndroidEnosys);
  }
  if (status != DARWIN_ART_BIONIC_IOCTL_FD_FOUND ||
      info.abi_version != DARWIN_ART_BIONIC_IOCTL_FD_INFO_ABI_VERSION) {
    return Fail(kAndroidEio);
  }
  if (request != kRndGetEntCnt ||
      info.kind != DARWIN_ART_BIONIC_IOCTL_FD_RANDOM_DEVICE) {
    return Fail(kAndroidEnotty);
  }
  if (!WriteInt32(argument, kRandomDeviceEntropyBits)) {
    return Fail(kAndroidEfault);
  }
  return 0;
}

}  // namespace

extern "C" int darwin_art_bionic_ioctl_bind_shared_memory(
    SharedMemoryIoctl callback) {
  if (callback == nullptr) return -1;
  SharedMemoryIoctl expected = nullptr;
  if (g_shared_memory_ioctl.compare_exchange_strong(
          expected, callback, std::memory_order_release,
          std::memory_order_acquire)) {
    return 0;
  }
  return expected == callback ? 0 : -1;
}

extern "C" DarwinArtBionicIoctlLifecycleStatus
darwin_art_bionic_ioctl_activate(DarwinArtBionicIoctlFdLookup lookup,
                                  void* context) {
  const int saved_errno = errno;
  DarwinArtBionicIoctlLifecycleStatus result =
      DARWIN_ART_BIONIC_IOCTL_LIFECYCLE_OK;
  (void)pthread_mutex_lock(&g_provider_lock);
  if (lookup == nullptr) {
    result = DARWIN_ART_BIONIC_IOCTL_LIFECYCLE_INVALID_ARGUMENT;
  } else if (g_provider.lookup != nullptr || g_provider.draining) {
    result = DARWIN_ART_BIONIC_IOCTL_LIFECYCLE_ALREADY_ACTIVE;
  } else {
    g_provider.lookup = lookup;
    g_provider.context = context;
  }
  (void)pthread_mutex_unlock(&g_provider_lock);
  errno = saved_errno;
  return result;
}

extern "C" DarwinArtBionicIoctlLifecycleStatus
darwin_art_bionic_ioctl_deactivate() {
  const int saved_errno = errno;
  (void)pthread_mutex_lock(&g_provider_lock);
  g_provider.draining = true;
  while (g_provider.in_flight != 0) {
    (void)pthread_cond_wait(&g_provider_condition, &g_provider_lock);
  }
  g_provider.lookup = nullptr;
  g_provider.context = nullptr;
  g_provider.draining = false;
  (void)pthread_mutex_unlock(&g_provider_lock);
  errno = saved_errno;
  return DARWIN_ART_BIONIC_IOCTL_LIFECYCLE_OK;
}

extern "C" int darwin_art_bionic_ioctl_captured(const uint64_t* registers) {
  const int saved_errno = errno;
  int result = -1;
  if (registers == nullptr) {
    result = Fail(kAndroidEfault);
  } else {
    result = Dispatch(static_cast<int32_t>(registers[0]),
                      static_cast<uint32_t>(registers[1]),
                      reinterpret_cast<void*>(registers[2]));
  }
  errno = saved_errno;
  return result;
}

extern "C" DarwinArtBionicIoctlFunction darwin_art_bionic_ioctl_resolve(
    const char* soname, const char* symbol, const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      strcmp(soname, "libc.so") != 0 || strcmp(symbol, "ioctl") != 0 ||
      strcmp(version, "LIBC") != 0) {
    return nullptr;
  }
  return reinterpret_cast<DarwinArtBionicIoctlFunction>(
      darwin_art_bionic_ioctl);
}

extern "C" const char* darwin_art_bionic_ioctl_capability(
    const char* capability) {
  if (capability == nullptr) return "invalid-capability";
  if (strcmp(capability, "android-aapcs64-varargs-capture") == 0 ||
      strcmp(capability, "rndgetentcnt") == 0 ||
      strcmp(capability, "virtual-fd-kind-seam") == 0) {
    return "supported";
  }
  return "unsupported";
}
