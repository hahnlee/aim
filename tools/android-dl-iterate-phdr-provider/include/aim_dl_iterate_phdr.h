#ifndef AIM_DL_ITERATE_PHDR_H_
#define AIM_DL_ITERATE_PHDR_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AIM_LOADED_IMAGE_SOURCE_ABI_VERSION 1u

typedef struct AimAndroidElf64Phdr {
  uint32_t p_type;
  uint32_t p_flags;
  uint64_t p_offset;
  uint64_t p_vaddr;
  uint64_t p_paddr;
  uint64_t p_filesz;
  uint64_t p_memsz;
  uint64_t p_align;
} AimAndroidElf64Phdr;

typedef struct AimAndroidDlPhdrInfo {
  uint64_t dlpi_addr;
  const char* dlpi_name;
  const AimAndroidElf64Phdr* dlpi_phdr;
  uint16_t dlpi_phnum;
  uint16_t reserved_padding[3];
  uint64_t dlpi_adds;
  uint64_t dlpi_subs;
  size_t dlpi_tls_modid;
  void* dlpi_tls_data;
} AimAndroidDlPhdrInfo;

typedef struct AimLoadedImageRecordV1 {
  uint64_t image_id;
  uint64_t generation;
  uint64_t load_bias;
  const char* soname;
  const AimAndroidElf64Phdr* phdrs;
  uint16_t phnum;
  uint16_t reserved16[3];
  size_t tls_modid;
  void* tls_data_for_current_thread;
} AimLoadedImageRecordV1;

typedef struct AimLoadedImageSnapshotV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void* lease;
  const AimLoadedImageRecordV1* records;
  size_t record_count;
  uint64_t load_events;
  uint64_t unload_events;
} AimLoadedImageSnapshotV1;

typedef int (*AimAcquireLoadedImageSnapshotV1)(
    void* context,
    AimLoadedImageSnapshotV1* snapshot_out);
typedef void (*AimReleaseLoadedImageSnapshotV1)(void* context,
                                                      void* lease);

typedef struct AimLoadedImageSourceV1 {
  uint32_t abi_version;
  uint32_t struct_size;
  void* context;
  AimAcquireLoadedImageSnapshotV1 acquire;
  AimReleaseLoadedImageSnapshotV1 release;
} AimLoadedImageSourceV1;

typedef int (*AimAndroidDlIterateCallback)(
    AimAndroidDlPhdrInfo* info,
    size_t info_size,
    void* data);

// Process-lifetime bind. The source table and context must outlive all Android
// images. Rebinding is rejected.
int aim_dl_phdr_bind_source(const AimLoadedImageSourceV1* source);

// Actual libdl.so@LIBC provider implementation.
int aim_bionic_dl_iterate_phdr(
    AimAndroidDlIterateCallback callback,
    void* data);

void* aim_dl_phdr_resolve(const char* soname,
                                 const char* symbol,
                                 const char* version);

#ifdef __cplusplus
}
#endif

#endif  // AIM_DL_ITERATE_PHDR_H_
