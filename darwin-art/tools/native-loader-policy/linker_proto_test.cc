#include "linkerconfig/configparser.h"
#include <algorithm>
#include <cassert>
#include <iostream>

int main(int argc, char** argv) {
  assert(argc == 2);
  auto parsed = android::linkerconfig::modules::ParseLinkerConfig(argv[1]);
  if (!parsed.ok()) { std::cerr << parsed.error() << '\n'; return 1; }
  const auto& config = *parsed;
  assert(config.providelibs_size() > 0);
  const auto& libs = config.providelibs();
  assert(std::find(libs.begin(), libs.end(), "libandroid.so") != libs.end());
  // A known regular input file cannot contain a child path: exercise the
  // original AOSP file-read error, not only the generated protobuf decoder.
  assert(!android::linkerconfig::modules::ParseLinkerConfig(
      std::string(argv[1]) + "/missing").ok());
  android::linkerconfig::proto::LinkerConfig invalid;
  assert(!invalid.ParseFromString(std::string("\xff", 1)));
  std::cout << "AOSP linker config parser PASS: provide=" << config.providelibs_size()
            << " require=" << config.requirelibs_size() << '\n';
}
