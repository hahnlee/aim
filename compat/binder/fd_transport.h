#pragma once

#include <stddef.h>
#include <stdint.h>

// This is a versioned provider port.  Keep it byte-for-byte compatible with
// descriptor_transport.rs; the lease is an opaque provider-owned object and
// is not part of the Binder payload.
enum { AIM_BINDER_DESCRIPTOR_ATTRIBUTES_BYTES = 256 };

typedef struct AimBinderTransferBinding {
  uint64_t source_connection;
  uint64_t transfer;
  uint64_t ordinal;
  uint64_t object_offset;
} AimBinderTransferBinding;

typedef struct AimBinderRetainedExportedDescriptor {
  int32_t host_fd;
  uint32_t attributes_length;
  uint8_t attributes[AIM_BINDER_DESCRIPTOR_ATTRIBUTES_BYTES];
  void *lease;
} AimBinderRetainedExportedDescriptor;

typedef struct AimBinderExportedDescriptor {
  int32_t host_fd;
  uint32_t attributes_length;
  uint8_t attributes[AIM_BINDER_DESCRIPTOR_ATTRIBUTES_BYTES];
} AimBinderExportedDescriptor;

#ifdef __cplusplus
static_assert(sizeof(AimBinderExportedDescriptor) == 264,
              "Binder bound descriptor ABI drift");
static_assert(sizeof(AimBinderTransferBinding) == 32,
              "Binder transfer binding ABI drift");
static_assert(offsetof(AimBinderRetainedExportedDescriptor, lease) ==
                  264,
              "Binder retained descriptor ABI drift");
static_assert(sizeof(AimBinderRetainedExportedDescriptor) == 272,
              "Binder retained descriptor size drift");
#endif

// Android Java FileDescriptor values name descriptors in the guest process
// table. AOSP libbinder RPC transports native Darwin descriptors over SCM_RIGHTS,
// so the Parcel JNI boundary must translate ownership in both directions.
extern "C" int aim_binder_export_file_descriptor(int guest_fd);
extern "C" int aim_binder_import_file_descriptor(int host_fd);
extern "C" int aim_binder_close_file_descriptor(int guest_fd);
extern "C" int aim_binder_duplicate_file_descriptor(int guest_fd);
extern "C" int aim_binder_export_retained_file_descriptor(
    int guest_fd, const AimBinderTransferBinding *binding,
    AimBinderRetainedExportedDescriptor *result);
extern "C" void aim_binder_release_export_lease(void *lease);
extern "C" int aim_binder_export_bound_file_descriptor(
    int guest_fd, const AimBinderTransferBinding *binding,
    AimBinderExportedDescriptor *result);
extern "C" int aim_binder_import_bound_file_descriptor(
    int host_fd, const AimBinderTransferBinding *binding,
    const uint8_t *attributes, size_t attributes_length);
