// P0 experiment 7b: can a Darwin arm64 process get memory below 4 GiB?
// ART on arm64 keeps its heap (32-bit compressed references) in the low
// 4 GiB, which the default 4 GiB __PAGEZERO occupies.
#include <errno.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>

int main(void) {
  setvbuf(stdout, NULL, _IONBF, 0);
  mach_vm_address_t a = 0;
  mach_vm_size_t sz = 0;
  vm_region_basic_info_data_64_t info;
  mach_msg_type_number_t cnt = VM_REGION_BASIC_INFO_COUNT_64;
  mach_port_t obj;
  kern_return_t kr = mach_vm_region(mach_task_self(), &a, &sz, VM_REGION_BASIC_INFO_64, (vm_region_info_t)&info, &cnt, &obj);
  printf("  first region reported: %#llx size %#llx (kr=%d)\n", a, sz, kr);
  // Try to drop __PAGEZERO and allocate in its place.
  kr = mach_vm_deallocate(mach_task_self(), 0x4000, 0x100000000ull - 0x4000);
  printf("  mach_vm_deallocate(0x4000 .. 4 GiB): %s\n", mach_error_string(kr));
  kr = mach_vm_protect(mach_task_self(), 0x10000000, 0x4000, FALSE, VM_PROT_READ);
  printf("  mach_vm_protect(0x10000000, READ): %s\n", mach_error_string(kr));
  mach_vm_address_t want = 0x10000000;
  kr = mach_vm_allocate(mach_task_self(), &want, 0x100000, VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE);
  printf("  mach_vm_allocate(0x10000000, FIXED|OVERWRITE): %s\n", mach_error_string(kr));
  void *m = mmap((void *)0x10000000, 0x100000, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON | MAP_FIXED, -1, 0);
  printf("  mmap MAP_FIXED 0x10000000: %s\n", m == MAP_FAILED ? strerror(errno) : "ok");
  return 0;
}
