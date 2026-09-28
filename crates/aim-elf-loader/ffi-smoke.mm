#include "aim_elf_loader.h"

#include <fcntl.h>
#include <unistd.h>

#include <cstdio>
#include <cstddef>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <thread>
#include <vector>

namespace {

static_assert(sizeof(AimElfStatus) == sizeof(int32_t));
static_assert(sizeof(AimElfErrorBuffer) == 24);
static_assert(sizeof(AimElfLoadOptions) == 24);
static_assert(sizeof(AimElfLifecycleCallbacks) == 32);
static_assert(sizeof(AimElfSymbolRequest) == 56);
static_assert(offsetof(AimElfSymbolRequest, needed_libraries) == 40);
static_assert(sizeof(AimElfGraphSource) == 24);

int g_provider_data = 11;

extern "C" int provider_value() { return 77; }

struct ResolverState {
  int requests = 0;
};

void SetCallbackError(AimElfErrorBuffer* error, const char* message) {
  if (error == nullptr) return;
  error->required = std::strlen(message) + 1;
  if (error->capacity == 0 || error->data == nullptr) return;
  const size_t count = error->required <= error->capacity
                           ? error->required - 1
                           : error->capacity - 1;
  std::memcpy(error->data, message, count);
  error->data[count] = '\0';
}

AimElfResolveStatus Resolve(void* context,
                                 const AimElfSymbolRequest* request,
                                 uintptr_t* out_address,
                                 AimElfErrorBuffer* error) {
  auto* state = static_cast<ResolverState*>(context);
  if (state == nullptr || request == nullptr || out_address == nullptr ||
      request->abi_version != AIM_ELF_ABI_VERSION) {
    SetCallbackError(error, "bad resolver arguments");
    return AIM_ELF_RESOLVE_ERROR;
  }
  ++state->requests;
  if (request->needed_library_count != 1 ||
      std::strcmp(request->needed_libraries[0],
                  "libaim_provider.so") != 0 ||
      request->version_soname == nullptr || request->version_name == nullptr ||
      std::strcmp(request->version_soname,
                  "libaim_provider.so") != 0 ||
      std::strcmp(request->version_name, "AIM_1") != 0 ||
      request->version_flags != 0 || request->version_hidden != 0) {
    SetCallbackError(error, "dependency/version contract mismatch");
    return AIM_ELF_RESOLVE_ERROR;
  }
  if (std::strcmp(request->symbol, "provider_value") == 0) {
    *out_address = reinterpret_cast<uintptr_t>(&provider_value);
    return AIM_ELF_RESOLVE_FOUND;
  }
  if (std::strcmp(request->symbol, "provider_data") == 0) {
    *out_address = reinterpret_cast<uintptr_t>(&g_provider_data);
    return AIM_ELF_RESOLVE_FOUND;
  }
  return AIM_ELF_RESOLVE_NOT_FOUND;
}

struct ErrorStorage {
  char bytes[512] = {};
  AimElfErrorBuffer value{bytes, sizeof(bytes), 0};

  void Reset() {
    bytes[0] = '\0';
    value.required = 0;
  }
};

int Fail(const char* operation, AimElfStatus status,
         const ErrorStorage& error) {
  std::fprintf(stderr, "%s: %s (%d): %s [required=%zu]\n", operation,
               aim_elf_status_name(status), status, error.bytes,
               error.value.required);
  return 1;
}

std::vector<uint8_t> Read(const char* path) {
  std::ifstream stream(path, std::ios::binary);
  return std::vector<uint8_t>(std::istreambuf_iterator<char>(stream), {});
}

void Write(const std::filesystem::path& path, const std::vector<uint8_t>& bytes) {
  std::ofstream stream(path, std::ios::binary | std::ios::trunc);
  stream.write(reinterpret_cast<const char*>(bytes.data()), bytes.size());
}

bool DiscoverExpected(const std::filesystem::path& directory,
                      const std::string& root,
                      const char* const* providers,
                      size_t provider_count,
                      AimElfStatus expected_status,
                      int expected_root_is_elf,
                      const char* error_substring,
                      size_t expected_count) {
  const int fd = open(directory.c_str(),
                      O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
  if (fd < 0) return false;
  ErrorStorage error;
  AimElfDiscoveredGraph* graph = nullptr;
  int root_is_elf = -1;
  const AimElfStatus status = aim_elf_discover_sibling_graph(
      fd, reinterpret_cast<const uint8_t*>(root.data()), root.size(), providers,
      provider_count, &root_is_elf, &graph, &error.value);
  close(fd);
  if (status != expected_status || root_is_elf != expected_root_is_elf) {
    Fail("negative discovery", status, error);
    std::fprintf(stderr, "root-is-elf=%d expected=%d status-expected=%d\n",
                 root_is_elf, expected_root_is_elf, expected_status);
    aim_elf_discovered_graph_destroy(&graph);
    return false;
  }
  if (error_substring != nullptr &&
      std::strstr(error.bytes, error_substring) == nullptr) {
    std::fprintf(stderr, "discovery error lacks expected text '%s': %s\n",
                 error_substring, error.bytes);
    aim_elf_discovered_graph_destroy(&graph);
    return false;
  }
  if (status != AIM_ELF_OK) {
    if (graph != nullptr) {
      std::fprintf(stderr, "failed discovery published a graph\n");
      aim_elf_discovered_graph_destroy(&graph);
      return false;
    }
    return true;
  }
  const AimElfGraphSource* sources = nullptr;
  size_t count = 0;
  error.Reset();
  if (aim_elf_discovered_graph_sources(graph, &sources, &count,
                                               &error.value) !=
          AIM_ELF_OK ||
      sources == nullptr || count != expected_count) {
    aim_elf_discovered_graph_destroy(&graph);
    return false;
  }
  aim_elf_discovered_graph_destroy(&graph);
  return graph == nullptr;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 8 || aim_elf_abi_version() != AIM_ELF_ABI_VERSION) {
    std::fprintf(stderr,
                 "usage: ffi-smoke POSITIVE.so IMPORT.so PARENT.so DEP_A.so DEP_B.so LIBCXX_ROOT.so LIBCXX.so\n");
    return 2;
  }

  ErrorStorage error;
  ResolverState resolver_state;
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION, Resolve,
                                  &resolver_state};
  AimElfHandle* imported = nullptr;
  AimElfStatus status =
      aim_elf_load_path(argv[2], &options, &imported, &error.value);
  if (status != AIM_ELF_OK) return Fail("load_path(import)", status, error);
  if (resolver_state.requests != 2) {
    std::fprintf(stderr, "resolver call/cache count mismatch\n");
    return 1;
  }
  status = aim_elf_run_initializers(imported, &error.value);
  if (status != AIM_ELF_OK) return Fail("init(import)", status, error);
  uintptr_t imported_address = 0;
  status = aim_elf_lookup(imported, "imported_value", &imported_address,
                                 &error.value);
  if (status != AIM_ELF_OK)
    return Fail("lookup(imported_value)", status, error);
  auto imported_value = reinterpret_cast<int (*)(void)>(imported_address);
  if (imported_value() != 165) {
    std::fprintf(stderr, "versioned provider execution mismatch\n");
    return 1;
  }

  AimElfStatus other_thread_status = AIM_ELF_OK;
  std::thread wrong_thread([&] {
    uintptr_t ignored = 0;
    ErrorStorage thread_error;
    other_thread_status = aim_elf_lookup(
        imported, "imported_value", &ignored, &thread_error.value);
  });
  wrong_thread.join();
  if (other_thread_status != AIM_ELF_OK) {
    std::fprintf(stderr, "cross-thread lookup failed\n");
    return 1;
  }
  AimElfStatus cross_thread_unload = AIM_ELF_OK;
  std::thread unloading_thread([&] {
    ErrorStorage thread_error;
    cross_thread_unload = aim_elf_unload(&imported, &thread_error.value);
  });
  unloading_thread.join();
  if (cross_thread_unload != AIM_ELF_OK || imported != nullptr) {
    std::fprintf(stderr, "cross-thread unload failed\n");
    return 1;
  }
  status = aim_elf_unload(&imported, &error.value);
  if (status != AIM_ELF_OK)
    return Fail("unload(import,idempotent)", status, error);

  const std::vector<uint8_t> positive = Read(argv[1]);
  if (positive.empty()) {
    std::fprintf(stderr, "failed to read positive fixture\n");
    return 1;
  }
  AimElfHandle* local = nullptr;
  status = aim_elf_load_bytes(positive.data(), positive.size(), nullptr,
                                     &local, &error.value);
  if (status != AIM_ELF_OK) return Fail("load_bytes", status, error);
  status = aim_elf_run_initializers(local, &error.value);
  if (status != AIM_ELF_OK) return Fail("init(local)", status, error);
  error.Reset();
  status = aim_elf_run_initializers(local, &error.value);
  if (status != AIM_ELF_LIFECYCLE || error.value.required == 0) {
    return Fail("init(local,twice)", status, error);
  }
  uintptr_t local_address = 0;
  status = aim_elf_lookup(local, "fixture_value", &local_address,
                                 &error.value);
  if (status != AIM_ELF_OK)
    return Fail("lookup(fixture_value)", status, error);
  auto fixture_value = reinterpret_cast<int (*)(void)>(local_address);
  if (fixture_value() != 42) {
    std::fprintf(stderr, "bytes-loaded fixture mismatch\n");
    return 1;
  }
  status = aim_elf_unload(&local, &error.value);
  if (status != AIM_ELF_OK) return Fail("unload(local)", status, error);

  AimElfHandle* unresolved = nullptr;
  error.Reset();
  status = aim_elf_load_path(argv[2], nullptr, &unresolved, &error.value);
  if (status != AIM_ELF_UNRESOLVED_SYMBOL || unresolved != nullptr ||
      error.value.required == 0) {
    return Fail("closed-default import rejection", status, error);
  }

  const std::vector<uint8_t> parent = Read(argv[3]);
  const std::vector<uint8_t> dep_a = Read(argv[4]);
  const std::vector<uint8_t> dep_b = Read(argv[5]);
  AimElfInspection* inspection = nullptr;
  error.Reset();
  status = aim_elf_inspect_bytes(parent.data(), parent.size(), &inspection,
                                        &error.value);
  if (status != AIM_ELF_OK || inspection == nullptr) {
    return Fail("inspect_bytes", status, error);
  }
  const char* inspected_soname = nullptr;
  size_t inspected_needed_count = 0;
  status = aim_elf_inspection_soname(inspection, &inspected_soname,
                                             &error.value);
  if (status != AIM_ELF_OK || inspected_soname == nullptr ||
      std::strcmp(inspected_soname, "libgraph_parent.so") != 0) {
    return Fail("inspection_soname", status, error);
  }
  status = aim_elf_inspection_needed_count(
      inspection, &inspected_needed_count, &error.value);
  if (status != AIM_ELF_OK || inspected_needed_count != 2) {
    return Fail("inspection_needed_count", status, error);
  }
  aim_elf_inspection_destroy(&inspection);
  if (inspection != nullptr) {
    std::fprintf(stderr, "inspection destroy did not clear ownership\n");
    return 1;
  }
  const AimElfGraphSource graph_sources[] = {
      {"libgraph_parent.so", parent.data(), parent.size()},
      {"libgraph_dep_a.so", dep_a.data(), dep_a.size()},
      {"libgraph_dep_b.so", dep_b.data(), dep_b.size()},
  };
  std::string parent_path(argv[3]);
  const size_t parent_slash = parent_path.find_last_of('/');
  const std::string parent_directory =
      parent_slash == std::string::npos ? "." : parent_path.substr(0, parent_slash);
  const std::string parent_component =
      parent_slash == std::string::npos ? parent_path : parent_path.substr(parent_slash + 1);
  const int directory_fd =
      open(parent_directory.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  if (directory_fd < 0) {
    std::perror("open discovery directory");
    return 1;
  }
  AimElfDiscoveredGraph* discovered = nullptr;
  int root_is_elf = 0;
  error.Reset();
  status = aim_elf_discover_sibling_graph(
      directory_fd, reinterpret_cast<const uint8_t*>(parent_component.data()),
      parent_component.size(), nullptr, 0, &root_is_elf, &discovered,
      &error.value);
  close(directory_fd);
  if (status != AIM_ELF_OK || root_is_elf != 1 || discovered == nullptr) {
    return Fail("discover_sibling_graph", status, error);
  }
  const char* discovered_root = nullptr;
  const AimElfGraphSource* discovered_sources = nullptr;
  size_t discovered_count = 0;
  status = aim_elf_discovered_graph_root_soname(
      discovered, &discovered_root, &error.value);
  if (status != AIM_ELF_OK || discovered_root == nullptr ||
      std::strcmp(discovered_root, "libgraph_parent.so") != 0) {
    return Fail("discovered_graph_root_soname", status, error);
  }
  status = aim_elf_discovered_graph_sources(
      discovered, &discovered_sources, &discovered_count, &error.value);
  if (status != AIM_ELF_OK || discovered_sources == nullptr ||
      discovered_count != 3) {
    return Fail("discovered_graph_sources", status, error);
  }
  aim_elf_discovered_graph_destroy(&discovered);
  if (discovered != nullptr) {
    std::fprintf(stderr, "discovery destroy did not clear ownership\n");
    return 1;
  }

  char temporary_template[] = "/tmp/aim-discovery.XXXXXX";
  const char* temporary_name = mkdtemp(temporary_template);
  if (temporary_name == nullptr) {
    std::perror("mkdtemp");
    return 1;
  }
  const std::filesystem::path temporary(temporary_name);
  struct TemporaryCleanup {
    std::filesystem::path path;
    ~TemporaryCleanup() { std::filesystem::remove_all(path); }
  } cleanup{temporary};
  auto reset_directory = [&] {
    std::filesystem::remove_all(temporary);
    std::filesystem::create_directory(temporary);
    std::filesystem::copy_file(argv[3], temporary / "libgraph_parent.so");
    std::filesystem::copy_file(argv[5], temporary / "libgraph_dep_b.so");
  };
  auto replace_needed = [&](std::vector<uint8_t>* bytes,
                            const std::vector<uint8_t>& replacement) {
    constexpr char kNeedle[] = "libgraph_dep_a.so";
    if (replacement.size() > sizeof(kNeedle) - 1) {
      return false;
    }
    bool replaced = false;
    auto position = bytes->begin();
    while (position != bytes->end()) {
      auto found = std::search(position, bytes->end(), std::begin(kNeedle),
                               std::end(kNeedle) - 1);
      if (found == bytes->end()) break;
      std::copy(replacement.begin(), replacement.end(), found);
      position = found + sizeof(kNeedle) - 1;
      replaced = true;
    }
    return replaced;
  };

  reset_directory();
  Write(temporary / "libgraph_dep_a.so", dep_b);
  if (!DiscoverExpected(temporary, "libgraph_parent.so", nullptr, 0,
                        AIM_ELF_FORMAT, 1,
                        "does not exactly match requested sibling", 0)) {
    return 1;
  }

  reset_directory();
  if (!DiscoverExpected(temporary, "libgraph_parent.so", nullptr, 0,
                        AIM_ELF_IO, 1, "component open failed", 0)) {
    return 1;
  }

  reset_directory();
  std::filesystem::create_symlink(argv[4], temporary / "libgraph_dep_a.so");
  if (!DiscoverExpected(temporary, "libgraph_parent.so", nullptr, 0,
                        AIM_ELF_IO, 1, "component open failed", 0)) {
    return 1;
  }

  if (!DiscoverExpected(temporary, "../libgraph_parent.so", nullptr, 0,
                        AIM_ELF_INVALID_ARGUMENT, 0,
                        "root ELF filename must be one nonempty byte component", 0)) {
    return 1;
  }

  reset_directory();
  std::vector<uint8_t> invalid_utf8_parent = parent;
  if (!replace_needed(&invalid_utf8_parent, {0xff, 'x', 0})) return 1;
  Write(temporary / "libgraph_parent.so", invalid_utf8_parent);
  if (!DiscoverExpected(temporary, "libgraph_parent.so", nullptr, 0,
                        AIM_ELF_FORMAT, 1,
                        "dependency filename is not UTF-8", 0)) {
    return 1;
  }

  reset_directory();
  std::vector<uint8_t> path_parent = parent;
  if (!replace_needed(&path_parent, {'.', '.', '/', 'x', 0})) return 1;
  Write(temporary / "libgraph_parent.so", path_parent);
  if (!DiscoverExpected(temporary, "libgraph_parent.so", nullptr, 0,
                        AIM_ELF_INVALID_ARGUMENT, 1,
                        "dependency filename must be one nonempty byte component", 0)) {
    return 1;
  }

  reset_directory();
  std::vector<uint8_t> libm_parent = parent;
  if (!replace_needed(&libm_parent, {'l', 'i', 'b', 'm', '.', 's', 'o', 0})) return 1;
  AimElfInspection* libm_inspection = nullptr;
  error.Reset();
  if (aim_elf_inspect_bytes(libm_parent.data(), libm_parent.size(),
                                   &libm_inspection, &error.value) !=
      AIM_ELF_OK) {
    return 1;
  }
  bool inspected_libm = false;
  for (size_t index = 0; index < 2; ++index) {
    const char* needed = nullptr;
    if (aim_elf_inspection_needed_at(libm_inspection, index, &needed,
                                            &error.value) !=
        AIM_ELF_OK) {
      return 1;
    }
    inspected_libm = inspected_libm ||
                     (needed != nullptr && std::strcmp(needed, "libm.so") == 0);
  }
  aim_elf_inspection_destroy(&libm_inspection);
  if (!inspected_libm) {
    std::fprintf(stderr, "libm provider mutation did not reach DT_NEEDED\n");
    return 1;
  }
  Write(temporary / "libgraph_parent.so", libm_parent);
  std::filesystem::copy_file(argv[4], temporary / "libgraph_dep_a.so");
  std::filesystem::create_symlink("/dev/null", temporary / "libm.so");
  const char* libm_provider[] = {"libm.so"};
  if (!DiscoverExpected(temporary, "libgraph_parent.so", libm_provider, 1,
                        AIM_ELF_OK, 1, nullptr, 3)) {
    return 1;
  }

  reset_directory();
  std::filesystem::resize_file(temporary / "libgraph_parent.so",
                               64u * 1024u * 1024u + 1u);
  if (!DiscoverExpected(temporary, "libgraph_parent.so", nullptr, 0,
                        AIM_ELF_BOUNDS, 1, "file cap", 0)) {
    return 1;
  }

  const std::filesystem::path real_directory = temporary / "real";
  std::filesystem::remove_all(temporary);
  std::filesystem::create_directories(real_directory);
  std::filesystem::create_directory_symlink(real_directory, temporary / "alias");
  const int symlink_directory_fd = open((temporary / "alias").c_str(),
                                        O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
  if (symlink_directory_fd >= 0) {
    close(symlink_directory_fd);
    std::fprintf(stderr, "trusted parent symlink was followed\n");
    return 1;
  }

  std::filesystem::remove_all(temporary);
  std::filesystem::create_directory(temporary);
  std::filesystem::copy_file(argv[6], temporary / "liblibcxx_discovery_root.so");
  std::filesystem::copy_file(argv[7], temporary / "libc++_shared.so");
  for (const char* provider : {"libc.so", "libdl.so", "libm.so"}) {
    std::filesystem::create_symlink("/dev/null", temporary / provider);
  }
  const char* libcxx_providers[] = {"libc.so", "libdl.so", "libm.so"};
  if (!DiscoverExpected(temporary, "liblibcxx_discovery_root.so",
                        libcxx_providers, std::size(libcxx_providers),
                        AIM_ELF_OK, 1, nullptr, 2)) {
    return 1;
  }
  AimElfGraphHandle* graph = nullptr;
  error.Reset();
  status = aim_elf_graph_load(
      "libgraph_parent.so", graph_sources, 1, nullptr, 0, nullptr, &graph,
      &error.value);
  if (status != AIM_ELF_UNRESOLVED_SYMBOL || graph != nullptr ||
      error.value.required == 0) {
    return Fail("graph_load(missing-dependency)", status, error);
  }
  error.Reset();
  status = aim_elf_graph_load(
      "libgraph_parent.so", graph_sources, std::size(graph_sources), nullptr,
      0, nullptr, &graph, &error.value);
  if (status != AIM_ELF_OK || graph == nullptr) {
    return Fail("graph_load", status, error);
  }
  uintptr_t graph_address = 0;
  status = aim_elf_graph_lookup_root(
      graph, "graph_value", &graph_address, &error.value);
  if (status != AIM_ELF_OK ||
      reinterpret_cast<int (*)(void)>(graph_address)() != 62) {
    return Fail("graph_lookup_root", status, error);
  }
  status = aim_elf_graph_unload(&graph, &error.value);
  if (status != AIM_ELF_OK || graph != nullptr) {
    return Fail("graph_unload", status, error);
  }
  status = aim_elf_graph_unload(&graph, &error.value);
  if (status != AIM_ELF_OK) {
    return Fail("graph_unload(idempotent)", status, error);
  }

  std::puts(
      "elf-loader-ffi-smoke: bytes=pass path=pass resolver=versioned "
      "lookup=pass init=lifecycle graph=atomic+recursive "
      "discovery=bytes+recursive+zero-provider+libcxx-r28c+fail-closed+caps+no-follow "
      "unload=idempotent thread=cross-thread+quiescent");
  return 0;
}
