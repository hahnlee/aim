#pragma once

#include <cstddef>
#include <memory>

namespace darwin_art::memory {

// System-owned storage with separate kernel read/write capabilities. No Java
// layout or lifecycle policy here. The system initializes the shared bytes;
// application binding exports only DuplicateReader().
class SystemRegion final {
 public:
  static std::unique_ptr<SystemRegion> Create(size_t size);
  ~SystemRegion();
  SystemRegion(const SystemRegion&) = delete;
  SystemRegion& operator=(const SystemRegion&) = delete;

  int DuplicateWriter() const;
  int DuplicateReader() const;
  size_t size() const { return size_; }

 private:
  SystemRegion(int writer, int reader, size_t size)
      : writer_(writer), reader_(reader), size_(size) {}
  const int writer_;
  const int reader_;
  const size_t size_;
};

}  // namespace darwin_art::memory
