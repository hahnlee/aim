#ifndef DARWIN_ART_BIONIC_VM_H_
#define DARWIN_ART_BIONIC_VM_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*DarwinArtBionicVmFunction)(void);
/* Resolver returns 1 for an ordinary host fd and 2 when a special device's
 * Android MAP_PRIVATE request requires a shared host view of device memory. */
typedef int (*DarwinArtBionicVmFdResolver)(int guest_fd, int* host_fd);

void* darwin_art_bionic_mmap(void*, size_t, int, int, int, int64_t);
void* darwin_art_bionic_mmap64(void*, size_t, int, int, int, int64_t);
int darwin_art_bionic_munmap(void*, size_t);
int darwin_art_bionic_mprotect(void*, size_t, int);
int darwin_art_bionic_madvise(void*, size_t, int);
void* darwin_art_bionic_mremap(void*, size_t, size_t, int, void*);
DarwinArtBionicVmFunction darwin_art_bionic_vm_resolve(const char*);
int darwin_art_bionic_vm_process_install(void);
int darwin_art_bionic_vm_process_uninstall(void);
int darwin_art_bionic_vm_recover_jit_execution_fault(uintptr_t fault_address,
                                                       int execution_fault);
/* A null callback clears the process filesystem descriptor bridge. */
int darwin_art_bionic_vm_bind_file_descriptor_resolver(
    DarwinArtBionicVmFdResolver resolver);
/* Registers loader-owned pages that guest mprotect may change, without
 * transferring unmap ownership to the VM facade. */
int darwin_art_bionic_vm_register_borrowed_range(void*, size_t);
int darwin_art_bionic_vm_unregister_borrowed_range(void*, size_t);

void* darwin_art_bionic_vm_mmap_core(void*, size_t, int, int, int, int64_t);
int darwin_art_bionic_vm_munmap_core(void*, size_t);
int darwin_art_bionic_vm_mprotect_core(void*, size_t, int);
int darwin_art_bionic_vm_madvise_core(void*, size_t, int);
void* darwin_art_bionic_vm_mremap_core(void*, size_t, size_t, int, void*);

size_t darwin_art_host_vm_page_size(void);
void* darwin_art_host_vm_map(void*, size_t, int, int, int, int64_t, int*);
int darwin_art_host_vm_unmap(void*, size_t, int*);
int darwin_art_host_vm_protect(void*, size_t, int, int*);
int darwin_art_host_vm_advise(void*, size_t, int, int*);
void* darwin_art_host_vm_remap_zero(void*, size_t, int, int*);
void darwin_art_host_vm_invalidate_icache(void*, size_t);
int darwin_art_host_vm_close_fd(int, int*);

#ifdef __cplusplus
}
#endif

#endif
