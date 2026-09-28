#include <cstdint>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>
#include <string>
#include <vector>
#include "dex/dex_file.h"
#include "dex/dex_file-inl.h"
#include "dex/dex_file_loader.h"

// Product build inspection is distinct from the fuzzer/smoke fixture entry.
int main(int argc, char** argv) {
  if (argc != 2) {
    std::cerr << "usage: dex-inspect classes.dex\n";
    return 2;
  }
  try {
    std::ifstream input(argv[1], std::ios::binary);
    if (!input.is_open()) { std::cerr << "could not open DEX\n"; return 2; }
    std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)),
                               std::istreambuf_iterator<char>());
    if (input.bad() || bytes.size() < sizeof(art::DexFile::Header)) {
      std::cerr << "DEX verification failed: incomplete header/read\n";
      return 1;
    }
    const size_t input_size = bytes.size();
    art::DexFileLoader loader(std::move(bytes), argv[1]);
    std::string error;
    auto dex_file = loader.OpenOne(0, 0, nullptr, /*verify=*/true, /*verify_checksum=*/true, &error);
    // Product support is one standard v38 DEX, never a container/prefix. The
    // last-entry predicate alone is tautological before the v41 container API.
    if (!dex_file || dex_file->GetDexVersion() != 38 || dex_file->Size() != input_size) {
      std::cerr << "DEX verification failed: " << error << '\n'; return 1;
    }
    const art::DexFile& dex = *dex_file;
    std::cout << "AOSP DEX: verified=yes version=" << dex.GetDexVersion()
              << " classes=" << dex.NumClassDefs() << " methods=" << dex.NumMethodIds();
    for (uint32_t index = 0; index < dex.NumClassDefs(); ++index)
      std::cout << " class[" << index << "]=" << dex.GetClassDescriptor(dex.GetClassDef(index));
    std::cout << '\n';
    return 0;
  } catch (const std::exception& error) {
    std::cerr << "DEX inspection failed: " << error.what() << '\n'; return 2;
  }
}
