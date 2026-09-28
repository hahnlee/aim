#include "loader/namespace_admission.h"
#include "loader/namespace_elf_group.h"
#include "loader/namespace_symbol_lookup.h"
#include "loader/namespace_operation.h"
#include "loader/namespace_load.h"
#include "darwin_android_elf_image_registry.h"
#include "loader/image_lifecycle.h"
#include "aim_bionic_vm.h"
#include <dlfcn.h>
#include <cassert>
#include <cstdio>
#include <fcntl.h>
#include <fstream>
#include <iterator>
#include <iostream>
#include <unistd.h>

namespace {
std::vector<uint8_t> Read(const std::string& path) {
  std::ifstream input(path, std::ios::binary);
  assert(input.good());
  return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
}
int NoOpen(const char*, char*, size_t, int*) { assert(false && "resident file reopen"); return -1; }
int NoErrno() { return 2; }
void CountDsoDestructor(void* value) { ++*static_cast<int*>(value); }
struct LifecycleState { int published = 0, finalized = 0, released = 0; };
namespace images = android::aim_image_registry;
struct LifecycleContext {
  std::shared_ptr<LifecycleState> state;
  std::shared_ptr<aim::loader::ImageLifecycle> images;
};
void* RetainLifecycle(void* input) { return new LifecycleContext(*static_cast<LifecycleContext*>(input)); }
void* RejectLifecycle(void*) { return nullptr; }
void ReleaseLifecycle(void* value) {
  auto* context = static_cast<LifecycleContext*>(value);
  ++context->state->released;
  delete context;
}
int PublishLifecycle(void* value, uintptr_t start, uintptr_t end) {
  assert(start < end);
  assert(static_cast<LifecycleContext*>(value)->images->Publish(start, end) == 0);
  ++static_cast<LifecycleContext*>(value)->state->published;
  return 0;
}
int FinalizeLifecycle(void* value, uintptr_t start, uintptr_t end) {
  assert(start < end);
  auto& state = *static_cast<LifecycleContext*>(value)->state;
  assert(state.released == 0 && state.published == 1);
  assert(static_cast<LifecycleContext*>(value)->images->Finalize(start, end) == 0);
  ++state.finalized;
  return 0;
}
}

void TestSymbolTraversal(const char*);
void TestVersionedSymbols(const char*);
void TestPageCompatExecution(const char*);
void TestResidentExecution(const char* fixture_directory) {
  using namespace aim::loader;
  assert(aim_bionic_vm_process_install() == 0);
  TestSymbolTraversal(fixture_directory);
  TestVersionedSymbols(fixture_directory);
  TestPageCompatExecution(fixture_directory);
  const std::string directory(fixture_directory);
  auto root_bytes = Read(directory + "/liblocal-root.so");
  auto child_bytes = Read(directory + "/liblocal-child.so");
  AimElfGraphSource sources[] = {
    {"liblocal-root.so", root_bytes.data(), root_bytes.size()},
    {"liblocal-child.so", child_bytes.data(), child_bytes.size()},
  };
  AimElfGraphHandle* original = nullptr;
  assert(aim_elf_graph_load("liblocal-root.so", sources, 2, nullptr, 0,
      nullptr, &original, nullptr) == AIM_ELF_OK);
  auto* registry = aim_linker_registry_create();
  uint64_t ns = 0;
  assert(aim_linker_namespace_create(registry, 0, nullptr, nullptr, nullptr, 0, &ns) == 0);
  std::string error;
  for (const char* name : {"liblocal-root.so", "liblocal-child.so"}) {
    AimElfSelectedImage* image = nullptr;
    assert(aim_elf_select_image(original, name, &image, nullptr) == AIM_ELF_OK);
    assert(PublishNamespaceElf(registry, ns, (std::string("/") + name).c_str(), image, false, &error));
    aim_elf_selected_image_release(image);
  }
  assert(aim_elf_graph_unload(&original, nullptr) == AIM_ELF_OK);
  {
    NamespaceOperation operation(registry);
    assert(operation);
    LinkerImageLease* image = nullptr;
    assert(aim_linker_namespace_find(registry, ns, "liblocal-root.so", &image) == 0);
    uintptr_t root_only = 0;
    assert(ResolveNamespaceElf(image, "child_value", nullptr, &root_only, &error) == 1);
    const auto child = LookupNamespaceSymbol(registry, image, "child_value", &error);
    assert(child && error.empty());
    assert(reinterpret_cast<int(*)()>(child)() == 42);
    assert(ResolveNamespaceElf(image, "parent_value", nullptr, &root_only, &error) == 0);
    assert(LookupNamespaceSymbol(registry, image, "parent_value", &error) == root_only);
    assert(!LookupNamespaceSymbol(registry, image, "absent_symbol_for_graph_test", &error));
    assert(!error.empty());
    aim_linker_image_release(image);
    std::puts("ELF handle symbols: original child export, root precedence, absence PASS");
  }
  auto* context = aim_linker_discovery_create(registry, ns, "/libconsumer.so", NoOpen, NoOpen, NoErrno);
  assert(context);
  DiscoveredGraph discovered;
  {
    NamespaceAdmission admission(context);
    const int fd = open((directory + "/libconsumer.so").c_str(), O_RDONLY | O_CLOEXEC);
    assert(fd >= 0);
    AimElfDiscoveredGraph* raw = nullptr;
    int is_elf = 0;
    char detail[2048]{};
    AimElfErrorBuffer output{detail, sizeof(detail), 0};
    auto status = aim_elf_discover_resident_graph_with_edges(fd, 1,
        reinterpret_cast<const uint8_t*>("libconsumer.so"), 14,
        AdmitNamespaceDependency, ReadNamespaceResidentDependencies,
        &admission, &is_elf, &raw, &output);
    close(fd);
    if (status != AIM_ELF_OK) std::cerr << detail << '\n';
    assert(status == AIM_ELF_OK && is_elf);
    discovered.reset(raw);
  }
  discovered.scopes = std::make_unique<NamespaceScopeSnapshot>();
  assert(discovered.scopes->Capture(discovered.get(), context, registry, ns, &error));
  aim_linker_discovery_destroy(context);
  size_t resident_count = 0, source_count = 0;
  const AimElfGraphSource* staged = nullptr;
  assert(aim_elf_discovered_graph_resident_count(discovered.get(), &resident_count, nullptr) == AIM_ELF_OK);
  assert(aim_elf_discovered_graph_sources(discovered.get(), &staged, &source_count, nullptr) == AIM_ELF_OK);
  assert(resident_count == 2 && source_count == 1); // No remapping of old images.
  AimElfGraphHandle* invalid = nullptr;
  assert(aim_elf_discovered_graph_load_with_owners(discovered.get(), nullptr, nullptr,
      nullptr, 0, nullptr, 0, &invalid, nullptr) != AIM_ELF_OK && !invalid);
  auto lifecycle_state = std::make_shared<LifecycleState>();
  AimElfLifecycleOwner* lifecycle = nullptr;
  {
    const char* residents[] = {"liblocal-root.so", "liblocal-child.so"};
    const char* invalid_paths[] = {"relative/libconsumer.so"};
    assert(!images::CreateWithPaths("libconsumer.so", staged, source_count,
        invalid_paths, residents, 2, &error));
    std::string admitted_path = "/data/app/installed/lib/arm64/consumer-renamed.so";
    const char* paths[] = {admitted_path.c_str()};
    ImageLifecycle::Registry image_owner(images::CreateWithPaths(
        "libconsumer.so", staged, source_count, paths, residents, 2, &error), images::Destroy);
    if (!image_owner) std::cerr << error << '\n';
    assert(image_owner);
    admitted_path.assign("/mutated/caller/storage.so");
    auto image_lifecycle = ImageLifecycle::Create(std::move(image_owner));
    assert(image_lifecycle);
    LifecycleContext original_context{lifecycle_state, std::move(image_lifecycle)};
    AimElfLifecycleCallbacks callbacks{AIM_ELF_ABI_VERSION,
        PublishLifecycle, FinalizeLifecycle, &original_context};
    assert(aim_elf_lifecycle_owner_create(&callbacks, nullptr, ReleaseLifecycle,
        &lifecycle, nullptr) != AIM_ELF_OK && !lifecycle);
    assert(aim_elf_lifecycle_owner_create(&callbacks, RejectLifecycle, ReleaseLifecycle,
        &lifecycle, nullptr) != AIM_ELF_OK && !lifecycle);
    callbacks.abi_version = 99;
    assert(aim_elf_lifecycle_owner_create(&callbacks, RetainLifecycle, ReleaseLifecycle,
        &lifecycle, nullptr) != AIM_ELF_OK && !lifecycle);
    assert(lifecycle_state->published == 0 && lifecycle_state->released == 0);
    callbacks.abi_version = AIM_ELF_ABI_VERSION;
    assert(aim_elf_lifecycle_owner_create(&callbacks, RetainLifecycle, ReleaseLifecycle,
        &lifecycle, nullptr) == AIM_ELF_OK);
  } // Original callback record and original context are both gone.
  auto loaded = LoadNamespaceGraph(std::move(discovered), nullptr, &error, lifecycle);
  aim_elf_lifecycle_owner_destroy(lifecycle);
  if (!loaded) std::cerr << error << '\n';
  assert(loaded);
  AimElfSelectedImage* consumer = nullptr;
  assert(aim_elf_select_image(loaded.get(), "libconsumer.so", &consumer, nullptr) == AIM_ELF_OK);
  void* original_dependency = nullptr;
  assert(aim_elf_selected_native_dependency(consumer, "liblocal-root.so", &original_dependency, nullptr) == AIM_ELF_OK);
  LinkerImageLease* expected = nullptr;
  assert(aim_linker_namespace_find(registry, ns, "liblocal-root.so", &expected) == 0);
  int32_t same = 0;
  assert(aim_linker_image_same(static_cast<LinkerImageLease*>(original_dependency), expected, &same) == 0 && same);
  void* private_dependency = nullptr;
  assert(aim_elf_selected_native_dependency(consumer, "liblocal-child.so", &private_dependency, nullptr) != AIM_ELF_OK && !private_dependency);
  aim_linker_registry_destroy(registry);
  assert(aim_linker_image_same(static_cast<LinkerImageLease*>(original_dependency), expected, &same) == 0 && same);
  aim_linker_image_release(expected);
  uintptr_t address = 0;
  assert(aim_elf_graph_lookup_root(loaded.get(), "consumer_value", &address, nullptr) == AIM_ELF_OK);
  assert(reinterpret_cast<int (*)()>(address)() == 43);
  int destructors = 0;
  assert(aim_bionic_dso_cxa_atexit_core(CountDsoDestructor,
      &destructors, reinterpret_cast<void*>(address)) == 0);
  Dl_info info{};
  assert(images::aim_android_elf_dladdr(reinterpret_cast<void*>(address), &info) != 0);
  assert(std::string(info.dli_fname) == "/data/app/installed/lib/arm64/consumer-renamed.so");
  loaded.graph.reset();
  assert(lifecycle_state->published == 1 && lifecycle_state->finalized == 0 && lifecycle_state->released == 0);
  assert(destructors == 0);
  assert(reinterpret_cast<int (*)()>(address)() == 43);
  assert(images::aim_android_elf_dladdr(reinterpret_cast<void*>(address), &info) != 0);
  assert(std::string(info.dli_fname) == "/data/app/installed/lib/arm64/consumer-renamed.so");
  aim_elf_selected_image_release(consumer);
  assert(lifecycle_state->finalized == 1 && lifecycle_state->released == 1);
  assert(destructors == 1);
  assert(images::aim_android_elf_dladdr(reinterpret_cast<void*>(address), &info) == 0);
  assert(aim_bionic_vm_process_uninstall() == 0);
  std::cout << "admitted ELF path copied through owned lifecycle and removed after final release PASS\n";
  std::cout << "owned lifecycle survives original context, owner handle and graph; final mapping releases once PASS\n";
  std::cout << "resident discovery -> production resolver/load -> retained execution result=43 PASS\n";
}
