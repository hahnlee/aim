#include <algorithm>
#include <filesystem>
#include <string>
#include <system_error>
#include <vector>

namespace {

constexpr char kText[] =
    "aim-libcxx-acceptance-crosses-the-small-string-boundary";

__attribute__((noinline)) const char* TextSource() {
  __asm__ __volatile__("" ::: "memory");
  return kText;
}

}  // namespace

extern "C" __attribute__((visibility("default"))) int
aim_libcxx_collections() {
  std::string text(TextSource());
  std::vector<int> values{9, 1, 7, 3, 5};
  std::sort(values.begin(), values.end());
  std::error_code error;
  const std::filesystem::path source("/libc++_shared.so");
  const std::filesystem::path destination("/data/libcxx-copy.so");
  const bool copied = std::filesystem::copy_file(
      source, destination, std::filesystem::copy_options::overwrite_existing,
      error);
  if (!copied || error) {
    return -10;
  }
  const std::uintmax_t source_size = std::filesystem::file_size(source, error);
  if (error) {
    return -11;
  }
  const std::uintmax_t destination_size =
      std::filesystem::file_size(destination, error);
  if (error || source_size == 0 || destination_size != source_size) {
    return -12;
  }
  return static_cast<int>(text.size()) +
         static_cast<unsigned char>(text.at(11)) + values.front() * 10 +
         values.back();
}

extern "C" __attribute__((visibility("default"))) int JNI_OnLoad(void*, void*) {
  return aim_libcxx_collections() == 189 ? 0x00010006 : -1;
}
