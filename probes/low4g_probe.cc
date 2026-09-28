#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <sys/mman.h>

#include <cerrno>
#include <cstdio>
#include <cstring>

int main() {
  constexpr mach_vm_size_t kSize = 64 * 1024 * 1024;

  constexpr mach_vm_address_t kCandidates[] = {
      0x0000000200010000ULL,
      0x0000000400010000ULL,
      0x0000001000010000ULL,
      0x0000010000010000ULL,
      0x0000100000010000ULL,
  };
  mach_vm_address_t reservation = 0;
  kern_return_t result = KERN_NO_SPACE;
  for (mach_vm_address_t candidate : kCandidates) {
    reservation = candidate;
    result = mach_vm_allocate(mach_task_self(), &reservation, kSize, VM_FLAGS_FIXED);
    std::printf("mach_vm_allocate: result=%d candidate=0x%llx address=0x%llx\n",
                result,
                static_cast<unsigned long long>(candidate),
                static_cast<unsigned long long>(reservation));
    if (result == KERN_SUCCESS) {
      break;
    }
  }
  if (result != KERN_SUCCESS) {
    return 1;
  }

  void* mapping = mmap(reinterpret_cast<void*>(reservation),
                       kSize,
                       PROT_READ | PROT_WRITE,
                       MAP_PRIVATE | MAP_ANON | MAP_FIXED,
                       -1,
                       0);
  std::printf("mmap: result=%p errno=%d (%s)\n", mapping, errno, std::strerror(errno));
  if (mapping == MAP_FAILED) {
    mach_vm_deallocate(mach_task_self(), reservation, kSize);
    return 2;
  }
  static_cast<volatile unsigned char*>(mapping)[0] = 42;
  munmap(mapping, kSize);
  return 0;
}
