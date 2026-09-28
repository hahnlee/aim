#include <androidfw/BigBuffer.h>
#include <androidfw/PathUtils.h>
#include <util/map_ptr.h>
#include <utils/String8.h>

#include <cstdint>
#include <iostream>
#include <string>
#include <utility>

int main() {
  android::BigBuffer buffer(8);
  auto* word = buffer.NextBlock<std::uint32_t>();
  *word = 0x44434241u;
  if (buffer.size() != sizeof(*word) || buffer.to_string() != "ABCD") {
    std::cerr << "androidfw BigBuffer failure\n";
    return 1;
  }

  const android::String8 path("/tmp/base.apk");
  if (std::string(android::getPathLeaf(path).c_str()) != "base.apk") {
    std::cerr << "androidfw PathUtils failure\n";
    return 2;
  }

  android::incfs::IncFsFileMap empty_map;
  android::incfs::IncFsFileMap moved_map(std::move(empty_map));
  (void)moved_map;

  std::cout << "androidfw host archive: BigBuffer+PathUtils+IncFsFileMap ok\n";
  return 0;
}
