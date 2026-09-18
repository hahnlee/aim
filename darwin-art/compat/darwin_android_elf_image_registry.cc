#include "darwin_android_elf_image_registry.h"

#include <unistd.h>
#include <signal.h>

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>

#include "darwin_art_dl_iterate_phdr.h"
extern "C" void darwin_art_bionic_dso_install_dladdr(
    int (*callback)(const void*, void*));

namespace android::darwin_art_image_registry {
namespace {

constexpr size_t kElfHeaderSize = 64;
constexpr size_t kProgramHeaderSize = 56;
constexpr uint32_t kPtLoad = 1;

uint16_t ReadU16(const uint8_t* bytes) {
  uint16_t value;
  std::memcpy(&value, bytes, sizeof(value));
  return value;
}

uint32_t ReadU32(const uint8_t* bytes) {
  uint32_t value;
  std::memcpy(&value, bytes, sizeof(value));
  return value;
}

uint64_t ReadU64(const uint8_t* bytes) {
  uint64_t value;
  std::memcpy(&value, bytes, sizeof(value));
  return value;
}

struct Image {
  uint64_t image_id = 0;
  uint64_t generation = 0;
  uint64_t load_bias = 0;
  uintptr_t start = 0;
  uintptr_t end = 0;
  std::string soname;
  std::string path;
  std::vector<DarwinArtAndroidElf64Phdr> phdrs;
};

struct SnapshotLease {
  std::vector<std::shared_ptr<const Image>> images;
  std::vector<DarwinArtLoadedImageRecordV1> records;
};

class Registry {
 public:
  uint64_t Publish(std::shared_ptr<Image> image) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (next_image_id_ == 0 || next_image_id_ == UINT64_MAX ||
        load_events_ == UINT64_MAX) {
      return 0;
    }
    const uint64_t image_id = next_image_id_;
    image->image_id = image_id;
    image->generation = image_id;
    images_.push_back(std::move(image));
    ++next_image_id_;
    ++load_events_;
    return image_id;
  }

  bool Unpublish(uint64_t image_id) {
    std::lock_guard<std::mutex> lock(mutex_);
    const auto found = std::find_if(
        images_.begin(), images_.end(), [image_id](const auto& image) {
          return image->image_id == image_id;
        });
    if (found == images_.end() || unload_events_ == UINT64_MAX) return false;
    images_.erase(found);
    ++unload_events_;
    return true;
  }

  int Acquire(DarwinArtLoadedImageSnapshotV1* snapshot) try {
    if (snapshot == nullptr) return -1;
    auto lease = std::make_unique<SnapshotLease>();
    uint64_t loads;
    uint64_t unloads;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      lease->images = images_;
      loads = load_events_;
      unloads = unload_events_;
    }
    lease->records.reserve(lease->images.size());
    for (const auto& image : lease->images) {
      DarwinArtLoadedImageRecordV1 record{};
      record.image_id = image->image_id;
      record.generation = image->generation;
      record.load_bias = image->load_bias;
      record.soname = image->soname.c_str();
      record.phdrs = image->phdrs.data();
      record.phnum = static_cast<uint16_t>(image->phdrs.size());
      lease->records.push_back(record);
    }
    snapshot->abi_version = DARWIN_ART_LOADED_IMAGE_SOURCE_ABI_VERSION;
    snapshot->struct_size = sizeof(*snapshot);
    snapshot->lease = lease.get();
    snapshot->records = lease->records.data();
    snapshot->record_count = lease->records.size();
    snapshot->load_events = loads;
    snapshot->unload_events = unloads;
    lease.release();
    return 0;
  } catch (...) {
    return -1;
  }

  int Lookup(uintptr_t address, void* opaque_info) {
    if (address == 0 || opaque_info == nullptr) return 0;
    struct DlInfo {
      const char* fname;
      void* fbase;
      const char* sname;
      void* saddr;
    };
    std::lock_guard<std::mutex> lock(mutex_);
    for (const auto& image : images_) {
      if (address < image->start || address >= image->end) continue;
      auto* info = static_cast<DlInfo*>(opaque_info);
      info->fname = image->path.c_str();
      info->fbase = reinterpret_cast<void*>(image->load_bias);
      info->sname = nullptr;
      info->saddr = nullptr;
      return 1;
    }
    return 0;
  }

 private:
  std::mutex mutex_;
  std::vector<std::shared_ptr<const Image>> images_;
  uint64_t next_image_id_ = 1;
  uint64_t load_events_ = 0;
  uint64_t unload_events_ = 0;
};

Registry& ProcessRegistry() {
  static Registry* registry = new Registry();
  return *registry;
}

int AcquireSnapshot(void* context,
                    DarwinArtLoadedImageSnapshotV1* snapshot) {
  return context == &ProcessRegistry() ? ProcessRegistry().Acquire(snapshot)
                                       : -1;
}

void ReleaseSnapshot(void* context, void* lease) {
  if (context != &ProcessRegistry() || lease == nullptr) std::abort();
  delete static_cast<SnapshotLease*>(lease);
}

bool BindProcessSource(std::string* error) {
  static std::once_flag once;
  static bool bound = false;
  std::call_once(once, [] {
    DarwinArtLoadedImageSourceV1 source{};
    source.abi_version = DARWIN_ART_LOADED_IMAGE_SOURCE_ABI_VERSION;
    source.struct_size = sizeof(source);
    source.context = &ProcessRegistry();
    source.acquire = &AcquireSnapshot;
    source.release = &ReleaseSnapshot;
    bound = darwin_art_dl_phdr_bind_source(&source) == 0;
    if (bound) {
      darwin_art_bionic_dso_install_dladdr(&darwin_art_android_elf_dladdr);
    }
  });
  if (!bound && error != nullptr) {
    *error = "Android dl_iterate_phdr snapshot source bind failed";
  }
  return bound;
}

struct PreparedImage {
  std::string soname;
  std::vector<DarwinArtAndroidElf64Phdr> phdrs;
  uint64_t minimum_page = 0;
  uint64_t mapping_size = 0;
};

bool ParseProgramHeaders(const DarwinArtElfGraphSource& source,
                         PreparedImage* prepared,
                         std::string* error) {
  if (source.soname == nullptr || source.bytes == nullptr ||
      source.length < kElfHeaderSize || prepared == nullptr) {
    if (error != nullptr) *error = "invalid ELF image-registry source";
    return false;
  }
  const uint8_t* bytes = source.bytes;
  if (std::memcmp(bytes, "\x7f" "ELF\x02\x01\x01", 7) != 0 ||
      ReadU16(bytes + 54) != kProgramHeaderSize) {
    if (error != nullptr) *error = "invalid ELF64 program-header identity";
    return false;
  }
  const uint64_t offset64 = ReadU64(bytes + 32);
  const uint16_t count = ReadU16(bytes + 56);
  if (count == 0 || offset64 > std::numeric_limits<size_t>::max()) {
    if (error != nullptr) *error = "invalid ELF program-header table";
    return false;
  }
  const size_t offset = static_cast<size_t>(offset64);
  if (offset > source.length ||
      count > (source.length - offset) / kProgramHeaderSize) {
    if (error != nullptr) *error = "truncated ELF program-header table";
    return false;
  }
  prepared->soname = source.soname;
  prepared->phdrs.reserve(count);
  uint64_t minimum_load = UINT64_MAX;
  uint64_t maximum_load = 0;
  for (size_t index = 0; index < count; ++index) {
    const uint8_t* entry = bytes + offset + index * kProgramHeaderSize;
    DarwinArtAndroidElf64Phdr phdr{};
    phdr.p_type = ReadU32(entry);
    phdr.p_flags = ReadU32(entry + 4);
    phdr.p_offset = ReadU64(entry + 8);
    phdr.p_vaddr = ReadU64(entry + 16);
    phdr.p_paddr = ReadU64(entry + 24);
    phdr.p_filesz = ReadU64(entry + 32);
    phdr.p_memsz = ReadU64(entry + 40);
    phdr.p_align = ReadU64(entry + 48);
    prepared->phdrs.push_back(phdr);
    if (phdr.p_type == kPtLoad) {
      minimum_load = std::min(minimum_load, phdr.p_vaddr);
      if (phdr.p_memsz > UINT64_MAX - phdr.p_vaddr) {
        if (error != nullptr) *error = "ELF PT_LOAD address overflow";
        return false;
      }
      maximum_load = std::max(maximum_load, phdr.p_vaddr + phdr.p_memsz);
    }
  }
  const long page_size = getpagesize();
  if (minimum_load == UINT64_MAX || page_size <= 0 ||
      (static_cast<uint64_t>(page_size) &
       (static_cast<uint64_t>(page_size) - 1)) != 0) {
    if (error != nullptr) *error = "ELF image has no valid PT_LOAD page";
    return false;
  }
  prepared->minimum_page =
      minimum_load & ~(static_cast<uint64_t>(page_size) - 1);
  const uint64_t page_mask = static_cast<uint64_t>(page_size) - 1;
  if (maximum_load > UINT64_MAX - page_mask) {
    if (error != nullptr) *error = "ELF PT_LOAD page span overflow";
    return false;
  }
  const uint64_t maximum_page = (maximum_load + page_mask) & ~page_mask;
  prepared->mapping_size = maximum_page - prepared->minimum_page;
  return true;
}

bool InspectNeeded(const DarwinArtElfGraphSource& source,
                   std::vector<std::string>* needed,
                   std::string* error) {
  DarwinArtElfInspection* inspection = nullptr;
  char message[512]{};
  DarwinArtElfErrorBuffer buffer{message, sizeof(message), 0};
  DarwinArtElfStatus status = darwin_art_elf_inspect_bytes(
      source.bytes, source.length, &inspection, &buffer);
  size_t count = 0;
  if (status == DARWIN_ART_ELF_OK) {
    status = darwin_art_elf_inspection_needed_count(inspection, &count, &buffer);
  }
  for (size_t index = 0; status == DARWIN_ART_ELF_OK && index < count;
       ++index) {
    const char* soname = nullptr;
    status = darwin_art_elf_inspection_needed_at(inspection, index, &soname,
                                                 &buffer);
    if (status == DARWIN_ART_ELF_OK && soname != nullptr) {
      needed->emplace_back(soname);
    }
  }
  darwin_art_elf_inspection_destroy(&inspection);
  if (status != DARWIN_ART_ELF_OK) {
    if (error != nullptr) {
      *error = std::string("ELF dependency inspection failed: ") + message;
    }
    return false;
  }
  return true;
}

}  // namespace

class Owner {
 public:
  struct PublishedImage {
    uint64_t image_id;
    uintptr_t start;
    uintptr_t end;
  };

  std::vector<PreparedImage> publication_order;
  std::string library_directory;
  std::unordered_map<std::string, std::string> admitted_paths;
  size_t next_publication = 0;
  std::vector<PublishedImage> published;
};

Owner* Create(const char* root_soname,
              const char* library_directory,
              const DarwinArtElfGraphSource* sources,
              size_t source_count,
              const char* const* provider_sonames,
              size_t provider_count,
              std::string* error) try {
  if (root_soname == nullptr || library_directory == nullptr ||
      library_directory[0] != '/' || sources == nullptr || source_count == 0 ||
      (provider_count != 0 && provider_sonames == nullptr) ||
      !BindProcessSource(error)) {
    if (error != nullptr && error->empty()) {
      *error = "invalid ELF image-registry graph";
    }
    return nullptr;
  }
  std::unordered_map<std::string, size_t> indices;
  std::vector<PreparedImage> prepared(source_count);
  std::vector<std::vector<std::string>> dependencies(source_count);
  for (size_t index = 0; index < source_count; ++index) {
    if (!ParseProgramHeaders(sources[index], &prepared[index], error) ||
        !InspectNeeded(sources[index], &dependencies[index], error) ||
        !indices.emplace(prepared[index].soname, index).second) {
      if (error != nullptr && error->empty()) {
        *error = "duplicate ELF image-registry SONAME";
      }
      return nullptr;
    }
  }
  const auto root = indices.find(root_soname);
  if (root == indices.end()) {
    if (error != nullptr) *error = "ELF image-registry root is absent";
    return nullptr;
  }
  std::unordered_set<std::string> providers;
  for (size_t index = 0; index < provider_count; ++index) {
    if (provider_sonames[index] == nullptr) {
      if (error != nullptr) *error = "null ELF provider SONAME";
      return nullptr;
    }
    providers.insert(provider_sonames[index]);
  }
  std::vector<size_t> order;
  std::vector<uint8_t> visiting(source_count, 0);
  const auto visit = [&](const auto& self, size_t index) -> bool {
    if (visiting[index] == 2) return true;
    if (visiting[index] == 1) return true;
    visiting[index] = 1;
    for (const std::string& dependency : dependencies[index]) {
      if (providers.contains(dependency)) continue;
      const auto found = indices.find(dependency);
      if (found == indices.end() || !self(self, found->second)) return false;
    }
    visiting[index] = 2;
    order.push_back(index);
    return true;
  };
  if (!visit(visit, root->second) || order.size() != source_count) {
    if (error != nullptr) {
      *error = "ELF image-registry dependency closure is incomplete";
    }
    return nullptr;
  }
  auto owner = std::make_unique<Owner>();
  owner->library_directory = library_directory;
  owner->publication_order.reserve(order.size());
  owner->published.reserve(order.size());
  for (size_t index : order) {
    owner->publication_order.push_back(std::move(prepared[index]));
  }
  return owner.release();
} catch (...) {
  if (error != nullptr) *error = "ELF image-registry allocation failed";
  return nullptr;
}

Owner* CreateWithPaths(const char* root_soname,
                      const DarwinArtElfGraphSource* sources, size_t source_count,
                      const char* const* source_paths,
                      const char* const* provider_sonames, size_t provider_count,
                      std::string* error) try {
  if (error) error->clear();
  if (!sources || !source_paths || !source_count) {
    if (error) *error = "missing admitted ELF source paths";
    return nullptr;
  }
  std::unordered_map<std::string, std::string> paths;
  for (size_t i = 0; i < source_count; ++i) {
    if (!sources[i].soname || !source_paths[i] || source_paths[i][0] != '/' ||
        !paths.emplace(sources[i].soname, source_paths[i]).second) {
      if (error) *error = "invalid or duplicate admitted ELF source path";
      return nullptr;
    }
  }
  // Create prepares metadata only; no image is published before paths are set.
  std::unique_ptr<Owner> owner(Create(root_soname, "/", sources, source_count,
                                     provider_sonames, provider_count, error));
  if (!owner) return nullptr;
  owner->admitted_paths = std::move(paths);
  return owner.release();
} catch (...) {
  if (error) *error = "ELF admitted-path allocation failed";
  return nullptr;
}

int Publish(Owner* owner, uintptr_t start, uintptr_t end) try {
  if (owner == nullptr || start >= end ||
      owner->next_publication >= owner->publication_order.size()) {
    return -1;
  }
  const PreparedImage& prepared =
      owner->publication_order[owner->next_publication];
  if (prepared.minimum_page > start || end - start != prepared.mapping_size ||
      prepared.phdrs.size() > std::numeric_limits<uint16_t>::max()) {
    return -1;
  }
  auto image = std::make_shared<Image>();
  image->load_bias = start - prepared.minimum_page;
  image->start = start;
  image->end = end;
  image->soname = prepared.soname;
  if (owner->admitted_paths.empty()) {
    image->path = owner->library_directory + "/" + prepared.soname;
  } else {
    const auto path = owner->admitted_paths.find(prepared.soname);
    if (path == owner->admitted_paths.end()) return -1;
    image->path = path->second;
  }
  image->phdrs = prepared.phdrs;
  if (std::getenv("DARWIN_ART_DEBUG_ELF_IMAGES") != nullptr) {
    std::fprintf(stderr,
                 "DARWIN ELF image: soname=%s start=%#lx end=%#lx "
                 "load_bias=%#lx pid=%ld\n",
                 image->soname.c_str(), static_cast<unsigned long>(start),
                 static_cast<unsigned long>(end),
                 static_cast<unsigned long>(image->load_bias),
                 static_cast<long>(getpid()));
  }
  if (std::getenv("DARWIN_ART_DEBUG_STOP_AT_CHROME") != nullptr &&
      image->soname == "libchrome.so") {
    // Diagnostic-only attach point. The image has been mapped and relocated,
    // but no Chromium entry point has run yet, so LLDB can install breakpoints
    // in the custom-loaded Android ELF text safely.
    (void)raise(SIGSTOP);
  }
  const uint64_t image_id = ProcessRegistry().Publish(std::move(image));
  if (image_id == 0) return -1;
  owner->published.push_back(Owner::PublishedImage{image_id, start, end});
  ++owner->next_publication;
  return 0;
} catch (...) {
  return -1;
}

extern "C" int darwin_art_android_elf_dladdr(const void* address,
                                               void* info) {
  return ProcessRegistry().Lookup(reinterpret_cast<uintptr_t>(address), info);
}

int RollbackPublish(Owner* owner, uintptr_t start, uintptr_t end) {
  if (owner == nullptr || owner->published.empty() ||
      owner->next_publication == 0) {
    return -1;
  }
  const Owner::PublishedImage& published = owner->published.back();
  if (published.start != start || published.end != end ||
      !ProcessRegistry().Unpublish(published.image_id)) {
    return -1;
  }
  owner->published.pop_back();
  --owner->next_publication;
  return 0;
}

int Finalize(Owner* owner, uintptr_t start, uintptr_t end) {
  if (owner == nullptr || owner->published.empty()) return -1;
  // Graph teardown follows the loader's dependency order, which is not
  // guaranteed to match publication order when PT_LOAD images overlap or a
  // dependency has its own finalizer.  Finalization must remove the exact
  // image being dropped rather than imposing a stack discipline that turns a
  // valid AOSP unload order into an abort.
  auto match = std::find_if(
      owner->published.begin(), owner->published.end(),
      [start, end](const Owner::PublishedImage& image) {
        return image.start == start && image.end == end;
      });
  if (match == owner->published.end() ||
      !ProcessRegistry().Unpublish(match->image_id)) {
    return -1;
  }
  owner->published.erase(match);
  return 0;
}

bool ContainsAddress(const Owner *owner, uintptr_t address) {
  if (owner == nullptr || address == 0)
    return false;
  for (const Owner::PublishedImage &image : owner->published) {
    if (address >= image.start && address < image.end)
      return true;
  }
  return false;
}

void UnpublishAll(Owner *owner) {
  if (owner == nullptr)
    return;
  for (auto it = owner->published.rbegin(); it != owner->published.rend();
       ++it) {
    if (!ProcessRegistry().Unpublish(it->image_id))
      std::abort();
  }
  owner->published.clear();
}

void Destroy(Owner* owner) {
  if (owner == nullptr) return;
  UnpublishAll(owner);
  delete owner;
}

}  // namespace android::darwin_art_image_registry
