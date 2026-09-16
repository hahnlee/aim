#include "darwin_art_elf_loader.h"

#include <fcntl.h>
#include <unistd.h>

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

namespace {

struct DependencyFiles { int directory; int calls = 0; };
int OpenDependency(void* context, uint64_t parent_image, const char* parent,
                   const char* name, const char* runpath, uint64_t* out_image) {
  if (runpath != nullptr) return -1; // These NDK fixtures contain no RUNPATH.
  auto* files = static_cast<DependencyFiles*>(context);
  const bool valid =
      (std::strcmp(parent, "libdarwin-art-generic-root.so") == 0 &&
       std::strcmp(name, "libdarwin-art-generic-child.so") == 0) ||
      (std::strcmp(parent, "libdarwin-art-generic-child.so") == 0 &&
       std::strcmp(name, "libdarwin-art-generic-grandchild.so") == 0);
  if (!valid || parent_image != static_cast<uint64_t>(41 + files->calls)) return -1;
  ++files->calls;
  *out_image = 41 + files->calls;
  return openat(files->directory, name, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
}

struct ErrorStorage {
  char bytes[512] = {};
  DarwinArtElfErrorBuffer value{bytes, sizeof(bytes), 0};
};

int Fail(const char* operation, DarwinArtElfStatus status,
         const ErrorStorage& error) {
  std::fprintf(stderr, "apk-native-discovery-smoke: %s: %s (%d): %s\n",
               operation, darwin_art_elf_status_name(status), status,
               error.bytes);
  return 1;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 3) {
    std::fprintf(stderr,
                 "usage: apk-native-discovery-smoke DIRECTORY ROOT_SONAME\n");
    return 64;
  }
  const int directory =
      open(argv[1], O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
  if (directory < 0) {
    std::perror("apk-native-discovery-smoke: open directory");
    return 1;
  }

  ErrorStorage error;
  // Inspect the real NDK root through an admitted descriptor, without changing
  // its shared offset. This exercises the C boundary, not just its byte reader.
  const int root_fd = openat(directory, argv[2], O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  if (root_fd < 0 || lseek(root_fd, 2, SEEK_SET) != 2) return 1;
  DarwinArtElfInspection* inspection = nullptr;
  const auto inspected = darwin_art_elf_inspect_fd(root_fd, &inspection, &error.value);
  if (inspected != DARWIN_ART_ELF_OK) return Fail("inspect admitted fd", inspected, error);
  if (inspection == nullptr || lseek(root_fd, 0, SEEK_CUR) != 2) return 1;
  size_t needed_count = 0;
  if (darwin_art_elf_inspection_needed_count(inspection, &needed_count, &error.value) != DARWIN_ART_ELF_OK || needed_count == 0) return 1;
  darwin_art_elf_inspection_destroy(&inspection);
  DarwinArtElfDiscoveredGraph* discovered = nullptr;
  int root_is_elf = 0;
  DarwinArtElfStatus status = darwin_art_elf_discover_sibling_graph(
      directory, reinterpret_cast<const uint8_t*>(argv[2]),
      std::strlen(argv[2]), nullptr, 0, &root_is_elf, &discovered,
      &error.value);
  if (status != DARWIN_ART_ELF_OK || discovered == nullptr || root_is_elf != 1) {
    darwin_art_elf_discovered_graph_destroy(&discovered);
    return Fail("discover", status, error);
  }
  darwin_art_elf_discovered_graph_destroy(&discovered);
  DependencyFiles files{directory};
  status = darwin_art_elf_discover_admitted_graph(
      root_fd, 41, reinterpret_cast<const uint8_t*>(argv[2]), std::strlen(argv[2]),
      nullptr, 0, OpenDependency, &files, &root_is_elf, &discovered, &error.value);
  if (status != DARWIN_ART_ELF_OK || discovered == nullptr || root_is_elf != 1 ||
      files.calls != 2 || lseek(root_fd, 0, SEEK_CUR) != 2) {
    return Fail("admitted graph", status, error);
  }
  DarwinArtElfDiscoveredGraph* rejected = nullptr;
  int rejected_is_elf = 0;
  const auto rejection = darwin_art_elf_discover_admitted_graph(
      root_fd, 41, reinterpret_cast<const uint8_t*>(argv[2]), std::strlen(argv[2]),
      nullptr, 0, nullptr, nullptr, &rejected_is_elf, &rejected, &error.value);
  if (rejection == DARWIN_ART_ELF_OK || rejected != nullptr || rejected_is_elf != 1 ||
      lseek(root_fd, 0, SEEK_CUR) != 2) return 1;
  close(root_fd);
  close(directory);

  const char* root = nullptr;
  const DarwinArtElfGraphSource* sources = nullptr;
  size_t count = 0;
  status = darwin_art_elf_discovered_graph_root_soname(discovered, &root,
                                                       &error.value);
  if (status != DARWIN_ART_ELF_OK || root == nullptr ||
      std::strcmp(root, argv[2]) != 0) {
    darwin_art_elf_discovered_graph_destroy(&discovered);
    return Fail("root-soname", status, error);
  }
  status = darwin_art_elf_discovered_graph_sources(discovered, &sources, &count,
                                                   &error.value);
  if (status != DARWIN_ART_ELF_OK || sources == nullptr || count != 3) {
    darwin_art_elf_discovered_graph_destroy(&discovered);
    return Fail("graph-sources", status, error);
  }

  DarwinArtElfGraphHandle* graph = nullptr;
  std::vector<std::pair<std::string, uint64_t>> expected_flags;
  for (size_t i = 0; i < count; ++i) {
    DarwinArtElfInspection* inspection = nullptr;
    uint64_t flags = 0;
    status = darwin_art_elf_inspect_bytes(sources[i].bytes, sources[i].length,
                                         &inspection, &error.value);
    if (status != DARWIN_ART_ELF_OK) return Fail("inspect flags", status, error);
    status = darwin_art_elf_inspection_flags_1(inspection, &flags, &error.value);
    darwin_art_elf_inspection_destroy(&inspection);
    if (status != DARWIN_ART_ELF_OK) return Fail("inspection flags", status, error);
    expected_flags.emplace_back(sources[i].soname, flags);
  }
  status = darwin_art_elf_graph_load(root, sources, count, nullptr, 0, nullptr,
                                     &graph, &error.value);
  darwin_art_elf_discovered_graph_destroy(&discovered);
  if (status != DARWIN_ART_ELF_OK || graph == nullptr) {
    darwin_art_elf_graph_unload(&graph, &error.value);
    return Fail("graph-load", status, error);
  }

  uintptr_t address = 0;
  for (const auto& entry : expected_flags) {
    uint64_t flags = UINT64_MAX;
    status = darwin_art_elf_graph_flags_1(graph, entry.first.c_str(), &flags, &error.value);
    if (status != DARWIN_ART_ELF_OK || flags != entry.second)
      return Fail("mapped flags", status, error);
  }
  uint64_t unchanged = 73;
  if (darwin_art_elf_graph_flags_1(graph, "absent.so", &unchanged, &error.value) ==
      DARWIN_ART_ELF_OK || unchanged != 73) return 1;
  status = darwin_art_elf_graph_lookup_root(graph, "JNI_OnLoad", &address,
                                            &error.value);
  if (status != DARWIN_ART_ELF_OK || address == 0) {
    darwin_art_elf_graph_unload(&graph, &error.value);
    return Fail("lookup JNI_OnLoad", status, error);
  }
  // This gate owns extraction, closed-namespace discovery, and symbol lookup.
  // The shared fixture's JNI_OnLoad intentionally dereferences a real
  // JavaVM/JNIEnv to test RegisterNatives, so invoking it with a sentinel VM
  // here would be undefined behavior. Managed-native-load owns that separate
  // execution contract.
  status = darwin_art_elf_graph_unload(&graph, &error.value);
  if (status != DARWIN_ART_ELF_OK || graph != nullptr) {
    return Fail("graph-unload", status, error);
  }
  std::puts(
      "apk-native-discovery-smoke: PASS extracted=APK graph=root+child+grandchild "
      "discovery=dirfd+admitted-fd+dependency-origin load=closed-no-provider JNI_OnLoad=located "
      "fallback=none");
  return 0;
}
