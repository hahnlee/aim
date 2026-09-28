#include "aim_dl_iterate_phdr.h"

#include <cstring>
#include <mutex>

namespace {

static_assert(sizeof(AimAndroidElf64Phdr) == 56);
static_assert(sizeof(AimAndroidDlPhdrInfo) == 64);
static_assert(offsetof(AimAndroidDlPhdrInfo, dlpi_adds) == 32);

std::mutex& SourceMutex() {
  static std::mutex* mutex = new std::mutex();
  return *mutex;
}

AimLoadedImageSourceV1& Source() {
  static AimLoadedImageSourceV1* source =
      new AimLoadedImageSourceV1{};
  return *source;
}

class SnapshotLease {
 public:
  SnapshotLease(const AimLoadedImageSourceV1& source,
                AimLoadedImageSnapshotV1 snapshot)
      : source_(source), snapshot_(snapshot) {}
  ~SnapshotLease() {
    if (snapshot_.lease != nullptr) {
      source_.release(source_.context, snapshot_.lease);
    }
  }
  const AimLoadedImageSnapshotV1& snapshot() const { return snapshot_; }

 private:
  AimLoadedImageSourceV1 source_;
  AimLoadedImageSnapshotV1 snapshot_;
};

bool ValidRecord(const AimLoadedImageRecordV1& record) {
  return record.image_id != 0 && record.generation != 0 &&
         record.soname != nullptr && record.phdrs != nullptr &&
         record.phnum != 0;
}

}  // namespace

extern "C" int aim_dl_phdr_bind_source(
    const AimLoadedImageSourceV1* source) {
  if (source == nullptr ||
      source->abi_version != AIM_LOADED_IMAGE_SOURCE_ABI_VERSION ||
      source->struct_size != sizeof(*source) || source->context == nullptr ||
      source->acquire == nullptr || source->release == nullptr) {
    return -1;
  }
  std::lock_guard<std::mutex> lock(SourceMutex());
  if (Source().acquire != nullptr) return -1;
  Source() = *source;
  return 0;
}

extern "C" int aim_bionic_dl_iterate_phdr(
    AimAndroidDlIterateCallback callback,
    void* data) {
  if (callback == nullptr) return -1;
  AimLoadedImageSourceV1 source{};
  {
    std::lock_guard<std::mutex> lock(SourceMutex());
    source = Source();
  }
  if (source.acquire == nullptr) return -1;
  AimLoadedImageSnapshotV1 snapshot{};
  if (source.acquire(source.context, &snapshot) != 0 ||
      snapshot.abi_version != AIM_LOADED_IMAGE_SOURCE_ABI_VERSION ||
      snapshot.struct_size != sizeof(snapshot) || snapshot.lease == nullptr ||
      (snapshot.record_count != 0 && snapshot.records == nullptr)) {
    if (snapshot.lease != nullptr) {
      source.release(source.context, snapshot.lease);
    }
    return -1;
  }
  SnapshotLease lease(source, snapshot);
  for (size_t index = 0; index < lease.snapshot().record_count; ++index) {
    const AimLoadedImageRecordV1& record = lease.snapshot().records[index];
    if (!ValidRecord(record)) return -1;
    AimAndroidDlPhdrInfo info{};
    info.dlpi_addr = record.load_bias;
    info.dlpi_name = record.soname;
    info.dlpi_phdr = record.phdrs;
    info.dlpi_phnum = record.phnum;
    info.dlpi_adds = lease.snapshot().load_events;
    info.dlpi_subs = lease.snapshot().unload_events;
    info.dlpi_tls_modid = record.tls_modid;
    info.dlpi_tls_data = record.tls_data_for_current_thread;
    const int result = callback(&info, sizeof(info), data);
    if (result != 0) return result;
  }
  return 0;
}

extern "C" void* aim_dl_phdr_resolve(const char* soname,
                                             const char* symbol,
                                             const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::strcmp(soname, "libdl.so") != 0 ||
      std::strcmp(symbol, "dl_iterate_phdr") != 0 ||
      std::strcmp(version, "LIBC") != 0) {
    return nullptr;
  }
  return reinterpret_cast<void*>(&aim_bionic_dl_iterate_phdr);
}
