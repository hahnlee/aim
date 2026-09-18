#pragma once

#include <cstdint>
#include <map>
#include <memory>
#include <span>

namespace darwin_art::surfaceflinger {

// Darwin storage import only; Android layer/transaction policy stays outside.
// Copies share the retained IOSurface. native_surface() is borrowed and valid
// only while at least one backing reference remains alive.
class IosurfaceBacking {
 public:
  static std::shared_ptr<const IosurfaceBacking> Import(uint32_t id);
  ~IosurfaceBacking();
  IosurfaceBacking(const IosurfaceBacking&) = delete;
  IosurfaceBacking& operator=(const IosurfaceBacking&) = delete;
  uint32_t id() const { return id_; }
  void* native_surface() const { return surface_; }

 private:
  IosurfaceBacking(uint32_t id, void* surface) : id_(id), surface_(surface) {}
  uint32_t id_;
  void* surface_;
};

// All requested nonzero storage IDs are imported before acknowledging a
// request. Failure drops the complete partial import; ID zero means no buffer.
class ImportedCompositionBackings {
 public:
  static std::shared_ptr<const ImportedCompositionBackings> Import(
      std::span<const uint32_t> ids);
  std::shared_ptr<const IosurfaceBacking> Find(uint32_t id) const;

 private:
  std::map<uint32_t, std::shared_ptr<const IosurfaceBacking>> surfaces_;
};

}  // namespace darwin_art::surfaceflinger
