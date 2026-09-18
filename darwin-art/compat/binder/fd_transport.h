#pragma once

#include <stddef.h>
#include <stdint.h>

// This is a versioned provider port.  Keep it byte-for-byte compatible with
// descriptor_transport.rs; the lease is an opaque provider-owned object and
// is not part of the Binder payload.
enum { DARWIN_ART_BINDER_DESCRIPTOR_ATTRIBUTES_BYTES = 256 };

typedef struct DarwinArtBinderTransferBinding {
  uint64_t source_connection;
  uint64_t transfer;
  uint64_t ordinal;
  uint64_t object_offset;
} DarwinArtBinderTransferBinding;

typedef struct DarwinArtBinderRetainedExportedDescriptor {
  int32_t host_fd;
  uint32_t attributes_length;
  uint8_t attributes[DARWIN_ART_BINDER_DESCRIPTOR_ATTRIBUTES_BYTES];
  void *lease;
} DarwinArtBinderRetainedExportedDescriptor;

#ifdef __cplusplus
static_assert(sizeof(DarwinArtBinderTransferBinding) == 32,
              "Binder transfer binding ABI drift");
static_assert(offsetof(DarwinArtBinderRetainedExportedDescriptor, lease) ==
                  264,
              "Binder retained descriptor ABI drift");
static_assert(sizeof(DarwinArtBinderRetainedExportedDescriptor) == 272,
              "Binder retained descriptor size drift");
#endif

// Android Java FileDescriptor values name descriptors in the guest process
// table. AOSP libbinder RPC transports native Darwin descriptors over SCM_RIGHTS,
// so the Parcel JNI boundary must translate ownership in both directions.
extern "C" int darwin_art_binder_export_file_descriptor(int guest_fd);
extern "C" int darwin_art_binder_import_file_descriptor(int host_fd);
extern "C" int darwin_art_binder_close_file_descriptor(int guest_fd);
extern "C" int darwin_art_binder_duplicate_file_descriptor(int guest_fd);
extern "C" int darwin_art_binder_export_retained_file_descriptor(
    int guest_fd, const DarwinArtBinderTransferBinding *binding,
    DarwinArtBinderRetainedExportedDescriptor *result);
extern "C" void darwin_art_binder_release_export_lease(void *lease);
