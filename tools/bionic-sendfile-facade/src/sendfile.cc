#include "aim_bionic_sendfile.h"

#include <errno.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>

extern "C" void aim_bionic_errno_store(int32_t android_errno);

namespace {

constexpr int32_t kAndroidEbadf = 9;
constexpr int32_t kAndroidEfault = 14;
constexpr int32_t kAndroidEio = 5;
constexpr int32_t kAndroidEinval = 22;
constexpr int32_t kAndroidEnosys = 38;

struct State {
  AimBionicSendfileTransfer transfer = nullptr;
  void* context = nullptr;
  size_t in_flight = 0;
  bool draining = false;
};

pthread_mutex_t g_lock = PTHREAD_MUTEX_INITIALIZER;
pthread_cond_t g_condition = PTHREAD_COND_INITIALIZER;
State g_state;

int Fail(int32_t value) {
  aim_bionic_errno_store(value);
  return -1;
}

bool AccessibleRange(const void* pointer, size_t length, vm_prot_t required) {
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
  return status == KERN_SUCCESS && (info.protection & required) == required &&
         region <= begin && region_size <= UINT64_MAX - region &&
         end <= region + region_size;
}

bool ReadOffset(const int64_t* pointer, int64_t* value) {
  if ((reinterpret_cast<uintptr_t>(pointer) & (alignof(int64_t) - 1)) != 0 ||
      !AccessibleRange(pointer, sizeof(*pointer), VM_PROT_READ)) {
    return false;
  }
  mach_vm_size_t copied = 0;
  return mach_vm_read_overwrite(
             mach_task_self(), reinterpret_cast<mach_vm_address_t>(pointer),
             sizeof(*value), reinterpret_cast<mach_vm_address_t>(value),
             &copied) == KERN_SUCCESS &&
         copied == sizeof(*value);
}

bool WriteOffset(int64_t* pointer, int64_t value) {
  if ((reinterpret_cast<uintptr_t>(pointer) & (alignof(int64_t) - 1)) != 0 ||
      !AccessibleRange(pointer, sizeof(*pointer), VM_PROT_READ | VM_PROT_WRITE)) {
    return false;
  }
  return mach_vm_write(mach_task_self(), reinterpret_cast<mach_vm_address_t>(pointer),
                       reinterpret_cast<vm_offset_t>(&value), sizeof(value)) ==
         KERN_SUCCESS;
}

bool Acquire(AimBionicSendfileTransfer* transfer, void** context) {
  (void)pthread_mutex_lock(&g_lock);
  if (g_state.transfer == nullptr || g_state.draining) {
    (void)pthread_mutex_unlock(&g_lock);
    return false;
  }
  ++g_state.in_flight;
  *transfer = g_state.transfer;
  *context = g_state.context;
  (void)pthread_mutex_unlock(&g_lock);
  return true;
}

void Release() {
  (void)pthread_mutex_lock(&g_lock);
  if (g_state.in_flight == 0) __builtin_trap();
  --g_state.in_flight;
  if (g_state.draining && g_state.in_flight == 0) {
    (void)pthread_cond_broadcast(&g_condition);
  }
  (void)pthread_mutex_unlock(&g_lock);
}

intptr_t Dispatch(int32_t output_fd, int32_t input_fd, int64_t* offset,
                  size_t count) {
  int64_t input_offset = 0;
  if (offset != nullptr && !ReadOffset(offset, &input_offset)) {
    return Fail(kAndroidEfault);
  }
  if (input_offset < 0) return Fail(kAndroidEinval);

  AimBionicSendfileTransfer transfer = nullptr;
  void* context = nullptr;
  if (!Acquire(&transfer, &context)) return Fail(kAndroidEnosys);
  const AimBionicSendfileRequest request{
      AIM_BIONIC_SENDFILE_ABI_VERSION,
      output_fd,
      input_fd,
      offset != nullptr ? 1U : 0U,
      input_offset,
      count,
  };
  AimBionicSendfileResult result{
      AIM_BIONIC_SENDFILE_ABI_VERSION, 0, -1, input_offset};
  const AimBionicSendfileTransferStatus status =
      transfer(context, &request, &result);
  Release();

  if (status == AIM_BIONIC_SENDFILE_TRANSFER_BAD_FD) {
    return Fail(kAndroidEbadf);
  }
  if (status == AIM_BIONIC_SENDFILE_TRANSFER_UNAVAILABLE) {
    return Fail(kAndroidEnosys);
  }
  if (status != AIM_BIONIC_SENDFILE_TRANSFER_OK ||
      result.abi_version != AIM_BIONIC_SENDFILE_ABI_VERSION ||
      result.transferred < -1 ||
      (result.transferred >= 0 &&
       static_cast<size_t>(result.transferred) > count) ||
      (result.transferred >= 0 && result.android_errno != 0) ||
      (result.transferred == -1 && result.android_errno <= 0)) {
    return Fail(kAndroidEio);
  }
  if (result.transferred == -1) return Fail(result.android_errno);
  if (offset != nullptr) {
    if (result.next_offset < input_offset ||
        static_cast<uint64_t>(result.next_offset - input_offset) !=
            static_cast<uint64_t>(result.transferred) ||
        !WriteOffset(offset, result.next_offset)) {
      return Fail(result.next_offset < input_offset ? kAndroidEio
                                                    : kAndroidEfault);
    }
  }
  return result.transferred;
}

}  // namespace

extern "C" AimBionicSendfileLifecycleStatus
aim_bionic_sendfile_activate(AimBionicSendfileTransfer transfer,
                                    void* context) {
  const int saved_errno = errno;
  AimBionicSendfileLifecycleStatus result =
      AIM_BIONIC_SENDFILE_LIFECYCLE_OK;
  (void)pthread_mutex_lock(&g_lock);
  if (transfer == nullptr) {
    result = AIM_BIONIC_SENDFILE_LIFECYCLE_INVALID_ARGUMENT;
  } else if (g_state.transfer != nullptr || g_state.draining) {
    result = AIM_BIONIC_SENDFILE_LIFECYCLE_ALREADY_ACTIVE;
  } else {
    g_state.transfer = transfer;
    g_state.context = context;
  }
  (void)pthread_mutex_unlock(&g_lock);
  errno = saved_errno;
  return result;
}

extern "C" AimBionicSendfileLifecycleStatus
aim_bionic_sendfile_deactivate() {
  const int saved_errno = errno;
  (void)pthread_mutex_lock(&g_lock);
  g_state.draining = true;
  while (g_state.in_flight != 0) {
    (void)pthread_cond_wait(&g_condition, &g_lock);
  }
  g_state.transfer = nullptr;
  g_state.context = nullptr;
  g_state.draining = false;
  (void)pthread_mutex_unlock(&g_lock);
  errno = saved_errno;
  return AIM_BIONIC_SENDFILE_LIFECYCLE_OK;
}

extern "C" intptr_t aim_bionic_sendfile(int output_fd, int input_fd,
                                                int64_t* offset,
                                                size_t count) {
  const int saved_errno = errno;
  const intptr_t result = Dispatch(output_fd, input_fd, offset, count);
  errno = saved_errno;
  return result;
}

extern "C" AimBionicSendfileFunction aim_bionic_sendfile_resolve(
    const char* soname, const char* symbol, const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      strcmp(soname, "libc.so") != 0 || strcmp(symbol, "sendfile") != 0 ||
      strcmp(version, "LIBC") != 0) {
    return nullptr;
  }
  return reinterpret_cast<AimBionicSendfileFunction>(
      aim_bionic_sendfile);
}
