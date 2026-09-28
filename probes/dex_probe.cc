#include <algorithm>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>
#include <string>
#include <vector>

#include "dex/dex_file.h"
#include "dex/dex_file-inl.h"
#include "dex/dex_file_verifier.h"
#include "dex/standard_dex_file.h"

static bool g_summary_only = false;

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  auto container = std::make_shared<art::MemoryDexFileContainer>(data, size);
  art::StandardDexFile dex(data,
                           "aim-probe.dex",
                           /*location_checksum=*/0,
                           /*oat_dex_file=*/nullptr,
                           std::move(container));

  std::string error;
  if (!dex.Init(&error)) {
    std::cerr << "DEX init failed: " << error << '\n';
    return 1;
  }
  if (!art::dex::Verify(&dex, dex.GetLocation().c_str(), /*verify_checksum=*/true, &error)) {
    std::cerr << "DEX verification failed: " << error << '\n';
    return 1;
  }

  std::cout << "AOSP DEX: verified=yes version=" << dex.GetDexVersion()
            << " classes=" << dex.NumClassDefs()
            << " methods=" << dex.NumMethodIds();
  uint32_t class_limit = g_summary_only ? std::min(1u, dex.NumClassDefs()) : dex.NumClassDefs();
  for (uint32_t index = 0; index < class_limit; ++index) {
    std::cout << " class[" << index << "]="
              << dex.GetClassDescriptor(dex.GetClassDef(index));
  }
  std::cout << '\n';
  return 0;
}

int main(int argc, char** argv) {
  bool summary = argc == 3 && std::string(argv[1]) == "--summary";
  if (argc != 2 && !summary) {
    std::cerr << "usage: dex-probe [--summary] classes.dex\n";
    return 2;
  }

  g_summary_only = summary;
  const char* path = argv[summary ? 2 : 1];
  std::ifstream input(path, std::ios::binary);
  if (!input.is_open()) {
    std::cerr << "could not open DEX: " << path << '\n';
    return 2;
  }
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)),
                             std::istreambuf_iterator<char>());
  if (bytes.empty()) {
    std::cerr << "could not read DEX: " << path << '\n';
    return 2;
  }
  return LLVMFuzzerTestOneInput(bytes.data(), bytes.size());
}
