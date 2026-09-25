// Host libbinder_ndk AParcel subset over the host android::Parcel.
//
// HWUI's Bitmap/Gainmap parcel JNI (libs/hwui/jni) is written against the NDK
// AParcel API. On Android that API is libbinder_ndk's parcel.cpp wrapping the
// Java Parcel's android::Parcel; this file provides the same wire semantics for
// the functions HWUI uses, so Bitmap parceling keeps the AOSP format and owner.
// binder_parcel.h includes <uchar.h>, which the host adapter toolchain
// resolves to the NDK sysroot copy; declare the pinned NDK C ABI here instead.
#include <android/binder_status.h>
#include <jni.h>

#include <android-base/unique_fd.h>
#include <android_os_Parcel.h>
#include <binder/Parcel.h>

#include <fcntl.h>

#include "fd_transport.h"

#include <limits>
#include <new>

extern "C" int darwin_art_bionic_fs_adopt_host_fd_core(int host_fd);
extern "C" int darwin_art_bionic_fs_dup_host_fd_core(int guest_fd, int* host_fd);

struct AParcel {
  explicit AParcel(android::Parcel* parcel) : parcel(parcel) {}
  android::Parcel* parcel;
};

// Android 16 include_ndk/android/binder_parcel.h.
typedef bool (*AParcel_byteArrayAllocator)(void* arrayData, int32_t length, int8_t** outBuffer);

namespace {

// libbinder_ndk PruneStatusT: binder_status_t values equal the status_t codes
// they name; anything else is reported as STATUS_UNKNOWN_ERROR.
binder_status_t Prune(android::status_t status) {
  switch (status) {
    case android::OK:
      return STATUS_OK;
    case android::NO_MEMORY:
      return STATUS_NO_MEMORY;
    case android::INVALID_OPERATION:
      return STATUS_INVALID_OPERATION;
    case android::BAD_VALUE:
      return STATUS_BAD_VALUE;
    case android::BAD_TYPE:
      return STATUS_BAD_TYPE;
    case android::NAME_NOT_FOUND:
      return STATUS_NAME_NOT_FOUND;
    case android::PERMISSION_DENIED:
      return STATUS_PERMISSION_DENIED;
    case android::NO_INIT:
      return STATUS_NO_INIT;
    case android::ALREADY_EXISTS:
      return STATUS_ALREADY_EXISTS;
    case android::DEAD_OBJECT:
      return STATUS_DEAD_OBJECT;
    case android::FAILED_TRANSACTION:
      return STATUS_FAILED_TRANSACTION;
    case android::BAD_INDEX:
      return STATUS_BAD_INDEX;
    case android::NOT_ENOUGH_DATA:
      return STATUS_NOT_ENOUGH_DATA;
    case android::WOULD_BLOCK:
      return STATUS_WOULD_BLOCK;
    case android::TIMED_OUT:
      return STATUS_TIMED_OUT;
    case android::UNKNOWN_TRANSACTION:
      return STATUS_UNKNOWN_TRANSACTION;
    case android::FDS_NOT_ALLOWED:
      return STATUS_FDS_NOT_ALLOWED;
    case android::UNEXPECTED_NULL:
      return STATUS_UNEXPECTED_NULL;
    default:
      return STATUS_UNKNOWN_ERROR;
  }
}

}  // namespace

extern "C" AParcel* AParcel_fromJavaParcel(JNIEnv* env, jobject parcel) {
  if (env == nullptr || parcel == nullptr) return nullptr;
  android::Parcel* native = android::parcelForJavaObject(env, parcel);
  if (native == nullptr) return nullptr;
  return new (std::nothrow) AParcel(native);
}

extern "C" void AParcel_delete(AParcel* parcel) { delete parcel; }

extern "C" bool AParcel_getAllowFds(const AParcel* parcel) {
  return parcel != nullptr && parcel->parcel->allowFds();
}

extern "C" binder_status_t AParcel_writeInt32(AParcel* parcel, int32_t value) {
  return parcel == nullptr ? STATUS_UNEXPECTED_NULL : Prune(parcel->parcel->writeInt32(value));
}

extern "C" binder_status_t AParcel_writeUint32(AParcel* parcel, uint32_t value) {
  return parcel == nullptr ? STATUS_UNEXPECTED_NULL : Prune(parcel->parcel->writeUint32(value));
}

extern "C" binder_status_t AParcel_writeInt64(AParcel* parcel, int64_t value) {
  return parcel == nullptr ? STATUS_UNEXPECTED_NULL : Prune(parcel->parcel->writeInt64(value));
}

extern "C" binder_status_t AParcel_writeFloat(AParcel* parcel, float value) {
  return parcel == nullptr ? STATUS_UNEXPECTED_NULL : Prune(parcel->parcel->writeFloat(value));
}

extern "C" binder_status_t AParcel_readInt32(const AParcel* parcel, int32_t* value) {
  if (parcel == nullptr || value == nullptr) return STATUS_UNEXPECTED_NULL;
  return Prune(parcel->parcel->readInt32(value));
}

extern "C" binder_status_t AParcel_readUint32(const AParcel* parcel, uint32_t* value) {
  if (parcel == nullptr || value == nullptr) return STATUS_UNEXPECTED_NULL;
  return Prune(parcel->parcel->readUint32(value));
}

extern "C" binder_status_t AParcel_readInt64(const AParcel* parcel, int64_t* value) {
  if (parcel == nullptr || value == nullptr) return STATUS_UNEXPECTED_NULL;
  return Prune(parcel->parcel->readInt64(value));
}

extern "C" binder_status_t AParcel_readFloat(const AParcel* parcel, float* value) {
  if (parcel == nullptr || value == nullptr) return STATUS_UNEXPECTED_NULL;
  return Prune(parcel->parcel->readFloat(value));
}

// libbinder_ndk WriteArray<int8_t>: int32 length (-1 for null) then in-place bytes.
extern "C" binder_status_t AParcel_writeByteArray(AParcel* parcel, const int8_t* array,
                                                  int32_t length) {
  if (parcel == nullptr) return STATUS_UNEXPECTED_NULL;
  if (array == nullptr && length > 0) return STATUS_UNEXPECTED_NULL;
  if (length < -1) return STATUS_BAD_VALUE;
  android::status_t status =
      parcel->parcel->writeInt32(array == nullptr ? -1 : length);
  if (status != android::OK || length <= 0) return Prune(status);
  void* data = parcel->parcel->writeInplace(static_cast<size_t>(length));
  if (data == nullptr) return STATUS_NO_MEMORY;
  memcpy(data, array, static_cast<size_t>(length));
  return STATUS_OK;
}

// libbinder_ndk ReadArray<int8_t>: the allocator receives -1 for a null array.
extern "C" binder_status_t AParcel_readByteArray(const AParcel* parcel, void* arrayData,
                                                 AParcel_byteArrayAllocator allocator) {
  if (parcel == nullptr || allocator == nullptr) return STATUS_UNEXPECTED_NULL;
  int32_t length = 0;
  android::status_t status = parcel->parcel->readInt32(&length);
  if (status != android::OK) return Prune(status);
  if (length < -1) return STATUS_BAD_VALUE;
  int8_t* array = nullptr;
  if (!allocator(arrayData, length, &array)) return STATUS_NO_MEMORY;
  if (length <= 0) return STATUS_OK;
  if (array == nullptr) return STATUS_NO_MEMORY;
  const void* data = parcel->parcel->readInplace(static_cast<size_t>(length));
  if (data == nullptr) return STATUS_NO_MEMORY;
  memcpy(array, data, static_cast<size_t>(length));
  return STATUS_OK;
}

// A nullable ParcelFileDescriptor parcelable: int32 presence, then a dup'd fd.
// NDK callers pass host descriptors; the Darwin Parcel carries guest
// descriptors (fd_transport.h), so ownership is converted at this boundary.
extern "C" binder_status_t AParcel_writeParcelFileDescriptor(AParcel* parcel, int fd) {
  if (parcel == nullptr) return STATUS_UNEXPECTED_NULL;
  if (fd < 0) {
    if (fd != -1) return STATUS_UNKNOWN_ERROR;
    return Prune(parcel->parcel->writeInt32(0));
  }
  const int host = fcntl(fd, F_DUPFD_CLOEXEC, 0);
  if (host < 0) return STATUS_BAD_VALUE;
  // adopt consumes the host duplicate even on failure.
  const int guest = darwin_art_bionic_fs_adopt_host_fd_core(host);
  if (guest < 0) return STATUS_NO_MEMORY;
  android::status_t status = parcel->parcel->writeInt32(1);
  if (status == android::OK) {
    status = parcel->parcel->writeParcelFileDescriptor(guest, true /*takeOwnership*/);
    if (status == android::OK) return STATUS_OK;
  }
  darwin_art_binder_close_file_descriptor(guest);
  return Prune(status);
}

extern "C" binder_status_t AParcel_readParcelFileDescriptor(const AParcel* parcel, int* fd) {
  if (parcel == nullptr || fd == nullptr) return STATUS_UNEXPECTED_NULL;
  int32_t present = 0;
  android::status_t status = parcel->parcel->readInt32(&present);
  if (status != android::OK) return Prune(status);
  if (present == 0) {
    *fd = -1;
    return STATUS_OK;
  }
  // The Parcel keeps ownership of the guest descriptor it holds.
  const int guest = parcel->parcel->readParcelFileDescriptor();
  if (guest < 0) return STATUS_BAD_VALUE;
  int host = -1;
  if (darwin_art_bionic_fs_dup_host_fd_core(guest, &host) != 1 || host < 0) {
    return STATUS_BAD_VALUE;
  }
  *fd = host;
  return STATUS_OK;
}
