#pragma once

// Android Java FileDescriptor values name descriptors in the guest process
// table. AOSP libbinder RPC transports native Darwin descriptors over SCM_RIGHTS,
// so the Parcel JNI boundary must translate ownership in both directions.
extern "C" int darwin_art_binder_export_file_descriptor(int guest_fd);
extern "C" int darwin_art_binder_import_file_descriptor(int host_fd);
extern "C" int darwin_art_binder_close_file_descriptor(int guest_fd);
extern "C" int darwin_art_binder_duplicate_file_descriptor(int guest_fd);
