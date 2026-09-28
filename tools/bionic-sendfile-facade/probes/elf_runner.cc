#include "aim_bionic_sendfile.h"
#include "aim_bionic_errno.h"
#include "aim_elf_loader.h"

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

namespace {
struct Owner { std::string source = "abcde"; size_t input = 0; std::string first; std::string second; };

AimBionicSendfileTransferStatus Transfer(
    void* context, const AimBionicSendfileRequest* request,
    AimBionicSendfileResult* result) {
  auto& owner = *static_cast<Owner*>(context);
  if (request->input_fd != 20) return AIM_BIONIC_SENDFILE_TRANSFER_BAD_FD;
  if (request->output_fd == 23) {
    *result = {AIM_BIONIC_SENDFILE_ABI_VERSION, 30, -1, request->offset};
    return AIM_BIONIC_SENDFILE_TRANSFER_OK;
  }
  if (request->output_fd != 21 && request->output_fd != 22)
    return AIM_BIONIC_SENDFILE_TRANSFER_BAD_FD;
  size_t position = request->has_explicit_offset ? static_cast<size_t>(request->offset) : owner.input;
  size_t count = request->count;
  if (request->output_fd == 21 && count > 3) count = 3;
  count = position < owner.source.size() ? std::min(count, owner.source.size() - position) : 0;
  (request->output_fd == 21 ? owner.first : owner.second).append(owner.source, position, count);
  if (!request->has_explicit_offset) owner.input += count;
  *result = {AIM_BIONIC_SENDFILE_ABI_VERSION, 0,
             static_cast<intptr_t>(count), request->offset + static_cast<int64_t>(count)};
  return AIM_BIONIC_SENDFILE_TRANSFER_OK;
}

AimElfResolveStatus Resolve(void*, const AimElfSymbolRequest* request,
                                  uintptr_t* address, AimElfErrorBuffer*) {
  if (!request || !address || !request->symbol) return AIM_ELF_RESOLVE_ERROR;
  if (auto function = aim_bionic_sendfile_resolve(
          request->version_soname, request->symbol, request->version_name)) {
    *address = reinterpret_cast<uintptr_t>(function);
    return AIM_ELF_RESOLVE_FOUND;
  }
  if (std::strcmp(request->symbol, "__errno") == 0) {
    *address = reinterpret_cast<uintptr_t>(&aim_bionic___errno);
    return AIM_ELF_RESOLVE_FOUND;
  }
  return AIM_ELF_RESOLVE_ERROR;
}
}

int main(int argc, char** argv) {
  if (argc != 2) return 2;
  std::ifstream input(argv[1], std::ios::binary);
  std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), {});
  Owner owner;
  if (aim_bionic_sendfile_activate(&Transfer, &owner) !=
      AIM_BIONIC_SENDFILE_LIFECYCLE_OK) return 3;
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, &Resolve, nullptr};
  AimElfHandle* image = nullptr; char message[512]{};
  AimElfErrorBuffer error{message, sizeof(message), 0};
  if (aim_elf_load_bytes(bytes.data(), bytes.size(), &options, &image, &error) != AIM_ELF_OK) {
    std::fprintf(stderr, "load: %s\n", message); return 4;
  }
  uintptr_t address = 0;
  if (aim_elf_lookup(image, "sendfile_fixture", &address, &error) != AIM_ELF_OK) return 5;
  const int result = reinterpret_cast<int (*)()>(address)();
  if (aim_elf_unload(&image, &error) != AIM_ELF_OK) return 6;
  if (aim_bionic_sendfile_deactivate() != AIM_BIONIC_SENDFILE_LIFECYCLE_OK) return 7;
  if (result != 42 || owner.first != "abcde" || owner.second != "bcde" || owner.input != 5) {
    std::fprintf(stderr, "fixture=%d first=%s second=%s input=%zu errno=%d\n",
                 result, owner.first.c_str(), owner.second.c_str(), owner.input,
                 aim_bionic_errno_load()); return 8;
  }
  std::puts("bionic-sendfile-facade: Android ELF PASS partial+EOF+offset+errno");
  return 0;
}
