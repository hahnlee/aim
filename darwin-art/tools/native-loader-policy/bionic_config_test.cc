#include "linker_config.h"
#include "loader/configured_namespaces.h"
#include "loader/namespace_handles.h"
#include "loader/bionic_provider_image.h"
#include "loader/namespace_startup.h"
#include "loader/process_namespaces.h"
#include "loader/process_namespace_installation.h"
#include "loader/system_native_images.h"
#include "darwin_art/darwin_art.h"
void TestProcessNamespaceInstallation(
    const darwin_art::loader::SharedBionicProviders&,
    std::span<const darwin_art::loader::NativeProviderPlacement>);
#include "loader/android_dlext_types.h"
#include <nativeloader/dlext_namespaces.h>
#include "loader/namespace_load.h"
#include "loader/admitted_provider_symbols.h"
#include "darwin_art_elf_loader.h"
#include "darwin_art_bionic_process_state.h"
#include "darwin_art_bionic_vm.h"
#include <nativeloader/native_loader.h>
#include "darwin_android_elf_image_registry.h"
#include <dlfcn.h>
#include <cassert>
#include <cerrno>
#include <fcntl.h>
#include <filesystem>
#include <iostream>
#include <thread>
#include <vector>
#include <unistd.h>
#include <sys/stat.h>
extern "C" int darwin_art_bionic_fs_process_install(int, const uint8_t*, size_t,
                                                    const uint8_t*, size_t);
extern "C" int darwin_art_bionic_fs_process_uninstall();
extern "C" int darwin_art_linker_dlclose(void*);
extern "C" void* darwin_art_linker_dlsym(void*, const char*);
void TestResidentExecution(const char*);
void TestTypedMachOSymbols();
void TestOriginalNamespacePolicy(darwin_art::loader::NamespaceHandles&);
void TestSystemLibandroid(darwin_art::loader::NamespaceHandles&);
void TestSystemAAudio(darwin_art::loader::NamespaceHandles&);
void TestSystemZlib(darwin_art::loader::NamespaceHandles&);
void TestSystemStdcxx(darwin_art::loader::NamespaceHandles&);
void TestSystemEgl(darwin_art::loader::NamespaceHandles&);
void TestSystemVulkan();
void TestSystemGles(darwin_art::loader::NamespaceHandles&);
void TestSystemNativeWindow(darwin_art::loader::NamespaceHandles&);
void TestSystemGraphicsNdk(darwin_art::loader::NamespaceHandles&);
void TestLibraryWarnings();
#include "loader/namespace_loader_abi.h"
void TestNamespaceLoaderAbi(darwin_art::loader::NamespaceHandles&);
void TestNamespaceSearchPaths(darwin_art::loader::NamespaceHandles&);
void TestAnonymousInitialization(darwin_art::loader::NamespaceHandles&);
int AuditSystemPreloads();
int AuditBootLibraries();
void TestTargetSdkPolicy();
void TestImageSdkSnapshot(darwin_art::loader::NamespaceHandles&);
void TestLinearSymbolPhase(darwin_art::loader::NamespaceHandles&);
void TestCallerSymbolAbi(darwin_art::loader::NamespaceHandles&);
void TestTypedJniLibrary(std::shared_ptr<darwin_art::loader::NamespaceHandles>);
void TestRuntimeVm(std::shared_ptr<darwin_art::loader::NamespaceHandles>);
void TestPageCompatPolicy(darwin_art::loader::NamespaceHandles&);
void TestSystemLibdlAndroid(darwin_art::loader::NamespaceHandles&);
int main(int argc, char** argv) {
  if (argc == 3 && std::string(argv[1]) == "--resident") {
    TestResidentExecution(argv[2]);
    return 0;
  }
  assert(argc == 2 || (argc == 3 && (std::string(argv[2]) == "--load" ||
                                   std::string(argv[2]) == "--public-load" ||
                                   std::string(argv[2]) == "--initialize" ||
                                   std::string(argv[2]) == "--audit-boot" ||
                                   std::string(argv[2]) == "--audit-preloads")));
  int fd = open(argv[1], O_RDONLY | O_DIRECTORY | O_CLOEXEC);
  assert(fd >= 0);
  const uint8_t slash[] = {'/'};
  assert(darwin_art_bionic_fs_process_install(fd, slash, 1, slash, 1) == 0);
  close(fd);
  assert(darwin_art_bionic_process_state_process_install() == 0);
  assert(darwin_art_bionic_vm_process_install() == 0);
  const Config* config = nullptr;
  std::string error;
  assert(Config::read_binary_config("/linkerconfig/ld.config.txt", "/system/bin/app_process64",
                                   false, false, &config, &error));
  assert(config && config->namespace_configs().size() == 5);
  auto namespaces = darwin_art::loader::ConfiguredNamespaces::Create(
      config->namespace_configs(), "", &error);
  assert(namespaces && error.empty());
  uint64_t id = 0;
  assert(darwin_art_linker_namespace_exported(namespaces->registry(), "com_android_i18n", &id) == 0);
  assert(id == namespaces->Find("com_android_i18n"));
  assert(!Config::read_binary_config("/linkerconfig/absent", "/system/bin/app_process64",
                                    false, false, &config, &error));
  assert(error.empty() && errno == ENOENT); // Original bionic intentionally omits ENOENT text.
  assert(!Config::read_binary_config("/system", "/system/bin/app_process64",
                                    false, false, &config, &error));
  assert(!error.empty());
  assert(namespaces->Find("default")); // No borrowed Config objects retained.
  auto loaded = darwin_art::loader::ConfiguredNamespaces::Read(
      "/linkerconfig/ld.config.txt", "/system/bin/app_process64", false, false, "", &error);
  assert(loaded && error.empty() && loaded->target_sdk_version() == 36);
  assert(loaded->Find("com_android_conscrypt"));
  assert(!darwin_art::loader::ConfiguredNamespaces::Read(
      "/linkerconfig/absent", "/system/bin/app_process64", false, false, "", &error));
  assert(!error.empty() && loaded->Find("default"));
  assert(!darwin_art::loader::ConfiguredNamespaces::Read(
      "relative", "/system/bin/app_process64", false, false, "", &error));
  // Each caller owns its registry even though upstream Config uses singleton
  // scratch storage. Exercise successful reads interleaved with parser resets.
  std::vector<std::thread> readers;
  for (int worker = 0; worker < 4; ++worker) {
    readers.emplace_back([] {
      for (int iteration = 0; iteration < 8; ++iteration) {
        std::string detail;
        auto owned = darwin_art::loader::ConfiguredNamespaces::Read(
            "/linkerconfig/ld.config.txt", "/system/bin/app_process64",
            false, false, "", &detail);
        assert(owned && detail.empty() && owned->target_sdk_version() == 36);
        assert(!darwin_art::loader::ConfiguredNamespaces::Read(
            "/linkerconfig/absent", "/system/bin/app_process64",
            false, false, "", &detail));
        uint64_t exported = 0;
        assert(darwin_art_linker_namespace_exported(
            owned->registry(), "com_android_i18n", &exported) == 0);
        assert(exported == owned->Find("com_android_i18n"));
      }
    });
  }
  for (auto& reader : readers) reader.join();
  // The Darwin runtime already owns these ported Bionic implementations.
  // Publish real sealed image owners into the configured default namespace;
  // APEX visibility is still decided by the original namespace links.
  const char* unwind_input = std::getenv("DARWIN_ART_TEST_ANDROID_UNWIND");
  assert(unwind_input && unwind_input[0] == '/');
  const std::string unwind_path(unwind_input);
  assert(!darwin_art::loader::SystemNativeImages::Create("", &error));
  assert(!error.empty());
  assert(!darwin_art::loader::SystemNativeImages::Create("relative-unwind.so", &error));
  assert(!error.empty());
  auto system_images = darwin_art::loader::SystemNativeImages::Create(unwind_path, &error);
  if (!system_images) std::cerr << "System native images: " << error << '\n';
  assert(system_images && error.empty());
  auto providers = system_images->providers();
  auto placements = system_images->placements();
  using darwin_art::loader::NativeProviderPlacement;
  assert(!android_get_exported_namespace("default"));
  assert(darwin_art_linker_dlerror());
  assert(!darwin_art::loader::InstallProcessNamespaces(nullptr));
  TestProcessNamespaceInstallation(providers, placements);
  static_assert(sizeof(darwin_art_native_loader_config_t) == 40);
  static_assert(offsetof(darwin_art_process_config_t, native_loader_config) == 128);
  darwin_art_native_loader_config_t native_config{
      sizeof(native_config), DARWIN_ART_NATIVE_LOADER_CONFIG_ABI_VERSION,
      "/linkerconfig/ld.config.txt", "/system/bin/app_process64",
      "", unwind_path.c_str()};
  auto invalid_config = native_config;
  invalid_config.struct_size = sizeof(native_config) - 1;
  assert(!darwin_art::loader::ProcessNamespaceInstallation::CreateFromConfig(&invalid_config, &error));
  assert(!darwin_art::loader::AcquireProcessNamespaces());
  invalid_config = native_config;
  invalid_config.abi_version = 99;
  assert(!darwin_art::loader::ProcessNamespaceInstallation::CreateFromConfig(&invalid_config, &error));
  assert(!darwin_art::loader::AcquireProcessNamespaces());
  invalid_config = native_config;
  invalid_config.executable_path = "relative-app-process";
  assert(!darwin_art::loader::ProcessNamespaceInstallation::CreateFromConfig(&invalid_config, &error));
  assert(!darwin_art::loader::AcquireProcessNamespaces());
  auto installation = darwin_art::loader::ProcessNamespaceInstallation::CreateFromConfig(
      &native_config, &error);
  assert(installation && error.empty());
  auto process = installation->namespaces();
  assert(!darwin_art::loader::InstallProcessNamespaces(process));
  TestOriginalNamespacePolicy(*process);
  TestTypedMachOSymbols();
  TestSystemVulkan();
  assert(android_get_exported_namespace("com_android_i18n") == process->Exported("com_android_i18n"));
  assert(!android_get_exported_namespace("missing"));
  assert(!android_get_exported_namespace(nullptr));
  assert(!darwin_art_linker_dlerror());
  {
    std::vector<std::thread> readers;
    auto* expected = process->Exported("com_android_i18n");
    for (int i = 0; i < 4; ++i) readers.emplace_back([expected] {
      for (int n = 0; n < 32; ++n)
        assert(android_get_exported_namespace("com_android_i18n") == expected);
    });
    for (auto& thread : readers) thread.join();
  }
  assert(!android_link_namespaces(nullptr, nullptr, "libc.so"));
  assert(darwin_art_linker_dlerror());
  char namespace_name[] = "abi-child";
  auto* abi_child = android_create_namespace(namespace_name, nullptr, "/data/app/lib",
      ANDROID_NAMESPACE_TYPE_ISOLATED, nullptr, process->Anonymous());
  assert(abi_child);
  namespace_name[0] = 'x';
  assert(process->Name(abi_child) == "abi-child");
  assert(!android_get_exported_namespace("abi-child"));
  assert(!android_create_namespace(nullptr, nullptr, nullptr, 0, nullptr, process->Anonymous()));
  assert(darwin_art_linker_dlerror());
  assert(!android_create_namespace("unsupported", nullptr, nullptr,
      ANDROID_NAMESPACE_TYPE_EXEMPT_LIST_ENABLED, nullptr, process->Anonymous()));
  assert(darwin_art_linker_dlerror());
  assert(!android_create_namespace("foreign", nullptr, nullptr, 0, nullptr,
      reinterpret_cast<android_namespace_t*>(uintptr_t{1})));
  assert(darwin_art_linker_dlerror());
  assert(android_link_namespaces(abi_child, nullptr, "libc.so"));
  LinkerImageLease* abi_libc = nullptr;
      assert(process->FindResident(abi_child, "libc.so", &abi_libc) == 0);
  darwin_art_linker_image_release(abi_libc);
  assert(!darwin_art::loader::ShutdownProcessNamespaces(nullptr));
  if (argc == 3) {
    auto concurrent = darwin_art::loader::CreateNamespaceRuntime(
        "/linkerconfig/ld.config.txt", "/system/bin/app_process64", false, false,
        "", providers, placements, &error);
    assert(concurrent);
    {
    auto concurrent = darwin_art::loader::CreateNamespaceRuntime(
        "/linkerconfig/ld.config.txt", "/system/bin/app_process64", false, false,
        "", providers, placements, &error);
    assert(concurrent);
    auto* target = concurrent->Anonymous();
    // Fresh path load must be placed in the directly linked i18n namespace,
    // not in an isolated caller merely because its link names libicu.so.
    auto* path_caller = concurrent->CreateChild(target, true, false, false,
        "/data/local/tmp", nullptr, nullptr, &error);
    assert(path_caller);
    assert(concurrent->Link(path_caller, concurrent->Exported("com_android_i18n"),
        "libicu.so", &error));
    LinkerImageLease* first_path = nullptr;
    assert(concurrent->OpenLocalPath(path_caller, "/apex/com.android.i18n/lib64/libicu.so",
        nullptr, &first_path, &error) < 0 && !first_path);
    const int path_result = concurrent->OpenPath(path_caller,
        "/apex/com.android.i18n/lib64/libicu.so", nullptr, &first_path, &error);
    if (path_result != 0) fprintf(stderr, "path admission/link failed: %s\n", error.c_str());
    assert(path_result == 0 && first_path);
    LinkerImageLease* placed = nullptr;
    assert(concurrent->FindResident(concurrent->Exported("com_android_i18n"), "libicu.so", &placed) == 0);
    int path_identity = 0;
    assert(darwin_art_linker_image_same(first_path, placed, &path_identity) == 0 && path_identity);
    darwin_art_linker_image_release(first_path);
    darwin_art_linker_image_release(placed);
    std::cerr << "fresh path load: direct linked namespace placement PASS\n";
    }
    auto* target = concurrent->Anonymous();
    LinkerImageLease* results[4]{};
    std::thread workers[4];
    for (size_t i = 0; i < 4; ++i) workers[i] = std::thread([&, i] {
      std::string detail;
      const int status = concurrent->OpenLocalSoname(target, "libicu.so", nullptr, &results[i], &detail);
      if (status != 0) std::cerr << "root open: " << detail << '\n';
      assert(status == 0 && results[i]);
    });
    for (auto& worker : workers) worker.join();
    DarwinArtLocalGroupMetadata original_group{};
    assert(darwin_art_linker_image_group_metadata(results[0], &original_group) == 0);
    assert(original_group.id != 0 && original_group.is_root == 1);
    uint8_t original_global = 0, original_nodelete = 0;
    assert(darwin_art_linker_image_retention_flags(results[0],
        &original_global, &original_nodelete) == 0);
    for (size_t i = 1; i < 4; ++i) {
      int same = 0;
      assert(darwin_art_linker_image_same(results[0], results[i], &same) == 0 && same);
      DarwinArtLocalGroupMetadata reused_group{};
      assert(darwin_art_linker_image_group_metadata(results[i], &reused_group) == 0);
      assert(reused_group.id == original_group.id && reused_group.is_root == 1);
      uint8_t root_global = 9, root_nodelete = 9;
      assert(darwin_art_linker_image_group_retention_flags(results[i],
          &root_global, &root_nodelete) == 0);
      assert(root_global == original_global && root_nodelete == original_nodelete);
    }
    LinkerImageLease* observed = nullptr;
    assert(concurrent->FindResident(target, "libicu.so", &observed) == 0);
    size_t open_count = 99;
    assert(darwin_art_linker_image_open_count(observed, &open_count) == 0 && open_count == 4);
    auto* shared_open = darwin_art_linker_image_clone(results[0]);
    assert(shared_open);
    for (auto* result : results) darwin_art_linker_image_release(result);
    assert(darwin_art_linker_image_open_count(observed, &open_count) == 0 && open_count == 1);
    darwin_art_linker_image_release(shared_open);
    assert(darwin_art_linker_image_open_count(observed, &open_count) == 0 && open_count == 0);
    darwin_art_linker_image_release(observed);
    std::cerr << "real ICU explicit opens: four opens, shared clone, balanced zero PASS\n";
    LinkerImageLease* missing = nullptr;
    assert(concurrent->OpenLocalSoname(target, "libdoesnotexist.so", nullptr, &missing, &error) == 1);
    assert(!missing);
    std::cerr << "real ICU concurrent root opens: one registry identity reused\n";
  }
  {
  const NativeProviderPlacement invalid[] = {
    placements[0], {"missing", "libdl.so", "/system/lib64/libdl.so"},
  };
  assert(!darwin_art::loader::CreateNamespaceRuntime(
      "/linkerconfig/ld.config.txt", "/system/bin/app_process64", false, false,
      "", providers, invalid, &error));
  assert(!error.empty()); // Failed replacement cannot invalidate runtime.
  }
  providers.reset(); // Namespace image leases, not this setup variable, own it.
  placements = {};
  system_images.reset();
  TestSystemLibandroid(*process);
  TestSystemAAudio(*process);
  TestSystemZlib(*process);
  TestSystemStdcxx(*process);
  TestSystemEgl(*process);
  TestSystemGles(*process);
  TestSystemNativeWindow(*process);
  TestSystemGraphicsNdk(*process);
  TestLibraryWarnings();
  TestNamespaceLoaderAbi(*process);
  TestNamespaceSearchPaths(*process);
  TestTargetSdkPolicy();
  TestImageSdkSnapshot(*process);
  TestLinearSymbolPhase(*process);
  TestCallerSymbolAbi(*process);
  TestTypedJniLibrary(process);
  TestRuntimeVm(process);
  TestPageCompatPolicy(*process);
  TestSystemLibdlAndroid(*process);
  if (argc == 3 && std::string(argv[2]) == "--audit-boot") {
    const int result = AuditBootLibraries();
    assert(installation->Shutdown());
    process.reset();
    loaded.reset();
    namespaces.reset();
    assert(darwin_art_bionic_fs_process_uninstall() == 0);
    assert(darwin_art_bionic_process_state_process_uninstall() == 0);
    assert(darwin_art_bionic_vm_process_uninstall() == 0);
    return result;
  }
  if (argc == 3 && std::string(argv[2]) == "--audit-preloads") {
    const int result = AuditSystemPreloads();
    assert(installation->Shutdown());
    process.reset();
    loaded.reset();
    namespaces.reset();
    assert(darwin_art_bionic_fs_process_uninstall() == 0);
    assert(darwin_art_bionic_process_state_process_uninstall() == 0);
    assert(darwin_art_bionic_vm_process_uninstall() == 0);
    return result;
  }
  if (argc == 3 && std::string(argv[2]) == "--initialize") {
    android::InitializeNativeLoader();
    android::ResetNativeLoader();
    std::cout << "original NativeLoader initialization/reset PASS\n";
    return 0;
  }
  {
    auto& handles = *process;
    darwin_art::loader::NamespaceFile file;
    // Real APEX ELF via generated namespace link + guest descriptor walk.
    assert(handles.OpenFile(handles.Anonymous(), "libicu.so", nullptr, nullptr, &file) == 0);
    assert(file.target == handles.Exported("com_android_i18n"));
    assert(file.canonical_path == "/apex/com.android.i18n/lib64/libicu.so");
    char magic[4] = {};
    assert(pread(file.fd.get(), magic, sizeof(magic), 0) == 4);
    assert(std::string(magic, 4) == std::string("\177ELF", 4));
    darwin_art::loader::DiscoveredGraph graph;
    const bool discovered = handles.Discover(file, "libicu.so", &graph, &error);
    std::cerr << "real ICU dependency discovery: " << (discovered ? "complete" : error) << '\n';
    // The materialized root must now satisfy the actual dependency graph.
    // Missing original system inputs remain failures, not passing baselines.
    assert(discovered && graph);
    size_t residents = 0;
    assert(darwin_art_elf_discovered_graph_resident_count(graph.get(), &residents, nullptr) == DARWIN_ART_ELF_OK);
    assert(residents > 0);
    bool has_libc = false;
    for (size_t i = 0; i < residents; ++i) {
      const char* name = nullptr;
      uint64_t image = 0;
      void* borrowed = nullptr;
      assert(darwin_art_elf_discovered_graph_resident(graph.get(), i,
          &name, &image, &borrowed, nullptr) == DARWIN_ART_ELF_OK);
      assert(name && image && borrowed);
      if (std::string(name) == "libc.so") {
        has_libc = true;
        const auto symbol = darwin_art::loader::ResolveBionicProviderImage(
            static_cast<LinkerImageLease*>(borrowed), "strlen", "LIBC");
        assert(symbol.status == DARWIN_ART_BIONIC_NAMESPACE_OK && symbol.address);
        assert(reinterpret_cast<size_t (*)(const char*)>(symbol.address)("namespace") == 9);
      }
    }
    assert(has_libc);
    uintptr_t address = 0;
    using darwin_art::loader::ResolveAdmittedProviderSymbol;
    assert(ResolveAdmittedProviderSymbol(graph.get(), "libc.so", "strlen",
        "LIBC", &address, &error) == 0);
    assert(reinterpret_cast<size_t (*)(const char*)>(address)("admitted") == 8);
    assert(ResolveAdmittedProviderSymbol(graph.get(), "libc.so", "strlen",
        "INVALID", &address, &error) == 1 && address == 0);
    assert(ResolveAdmittedProviderSymbol(graph.get(), "libc.so", "not_exported",
        nullptr, &address, &error) == 1 && address == 0);
    // A supported provider SONAME is not enough: this graph did not admit it.
    assert(ResolveAdmittedProviderSymbol(graph.get(), "libaaudio.so", "AAudio_createStreamBuilder",
        nullptr, &address, &error) == 1 && address == 0);
    const char* selected[] = {"libc.so"};
    DarwinArtElfSymbolRequest request{DARWIN_ART_ELF_ABI_VERSION, "strlen",
        nullptr, nullptr, 0, 0, 0, selected, 1};
    DarwinArtElfLoadOptions options{DARWIN_ART_ELF_ABI_VERSION,
        darwin_art::loader::ResolveAdmittedProvider, graph.get()};
    assert(options.resolver(options.resolver_context, &request, &address, nullptr) == DARWIN_ART_ELF_RESOLVE_FOUND);
    assert(reinterpret_cast<size_t (*)(const char*)>(address)("callback") == 8);
    request.symbol = "not_exported";
    assert(options.resolver(options.resolver_context, &request, &address, nullptr) == DARWIN_ART_ELF_RESOLVE_NOT_FOUND && !address);
    request.symbol = "strlen";
    request.version_soname = "libm.so";
    request.version_name = "LIBC";
    char short_error[8];
    DarwinArtElfErrorBuffer callback_error{short_error, sizeof(short_error), 0};
    assert(options.resolver(options.resolver_context, &request, &address, &callback_error) == DARWIN_ART_ELF_RESOLVE_ERROR && !address);
    assert(callback_error.required > sizeof(short_error) && short_error[7] == '\0');
    request.version_soname = request.version_name = nullptr;
    request.needed_library_count = 0;
    assert(options.resolver(options.resolver_context, &request, &address, nullptr) == DARWIN_ART_ELF_RESOLVE_ERROR && !address);
    const DarwinArtElfGraphSource* sources = nullptr;
    size_t source_count = 0;
    assert(darwin_art_elf_discovered_graph_sources(graph.get(), &sources,
        &source_count, nullptr) == DARWIN_ART_ELF_OK);
    assert(source_count > 0);
    assert(graph.placements.size() == source_count);
    assert(graph.placements.front().canonical_path == file.canonical_path);
    struct stat admitted_stat{};
    assert(fstat(file.fd.get(), &admitted_stat) == 0);
    assert(graph.placements.front().file.device == static_cast<uint64_t>(admitted_stat.st_dev));
    assert(graph.placements.front().file.inode == admitted_stat.st_ino);
    assert(graph.placements.front().file.offset == 0);
    for (size_t i = 0; i < source_count; ++i) {
      assert(std::string(sources[i].soname) != "libc.so");
      assert(graph.placements[i].namespace_id != 0);
      assert(graph.placements[i].soname == sources[i].soname);
      assert(graph.placements[i].canonical_path.starts_with("/"));
    }
    if (argc == 3 && std::string(argv[2]) == "--load") {
      auto mapped = darwin_art::loader::LinkNamespaceGraph(std::move(graph), nullptr, &error);
      std::cerr << "real ICU graph load: " << (mapped ? "complete" : error) << '\n';
      assert(mapped); // Full-load mode must not call discovery alone a pass.
      assert(mapped.placements.size() == source_count);
      assert(mapped.placements.front().canonical_path == file.canonical_path);
      DarwinArtElfSelectedImage* icu_image = nullptr;
      assert(darwin_art_elf_select_image(mapped.get(), "libicuuc.so", &icu_image, nullptr) == DARWIN_ART_ELF_OK);
      LinkerImageLease* published = nullptr;
      const auto last_namespace = mapped.placements.back().namespace_id;
      mapped.placements.back().namespace_id = 0;
      assert(!handles.Publish(mapped, false, &error));
      assert(handles.FindResident(handles.Exported("com_android_i18n"), "libicuuc.so", &published) == 1);
      assert(!published);
      mapped.placements.back().namespace_id = last_namespace;
      assert(handles.PublishAndInitialize(mapped, false, &error));
      LinkerImageLease* file_match = nullptr;
      assert(handles.FindResidentFile(handles.Anonymous(), mapped.placements.front().file, &file_match) == 0);
      LinkerImageLease* name_match = nullptr;
      assert(handles.FindResident(handles.Anonymous(), "libicu.so", &name_match) == 0);
      int file_same = 0;
      assert(darwin_art_linker_image_same(file_match, name_match, &file_same) == 0 && file_same);
      darwin_art_linker_image_release(file_match);
      darwin_art_linker_image_release(name_match);
      // A duplicate initialization must roll back only its new publication,
      // leaving the successful original registry image untouched.
      assert(handles.FindResident(handles.Exported("com_android_i18n"), "libicuuc.so", &published) == 0);
      LinkerImageLease* successful = published;
      assert(!handles.PublishAndInitialize(mapped, false, &error));
      assert(handles.FindResident(handles.Exported("com_android_i18n"), "libicuuc.so", &published) == 0);
      int retained_same = 0;
      assert(darwin_art_linker_image_same(successful, published, &retained_same) == 0 && retained_same);
      darwin_art_linker_image_release(successful);
      darwin_art_linker_image_release(published);
      published = nullptr;
      mapped.graph.reset(); // Registry publication owns the mapping now.
      assert(handles.FindResident(handles.Exported("com_android_i18n"), "libicuuc.so", &published) == 0);
      void* selected = nullptr;
      assert(darwin_art_linker_image_typed_payload(published, DARWIN_ART_IMAGE_ELF_SELECTED, &selected) == 0);
      int same = 0;
      assert(darwin_art_elf_selected_image_same(icu_image,
          static_cast<DarwinArtElfSelectedImage*>(selected), &same, nullptr) == DARWIN_ART_ELF_OK && same);
      darwin_art_linker_image_release(published);
      std::cerr << "real ICU graph publication: original mapped identity retained\n";
      // An explicit path is admitted and canonicalized by the guest VFS before
      // resident identity lookup.  The anonymous namespace is the selected
      // caller namespace here; its link to i18n grants libicu.so visibility.
      LinkerImageLease* path_alias = nullptr;
      assert(handles.OpenLocalPath(handles.Anonymous(),
          "/apex/com.android.i18n/lib64/./libicu.so", nullptr, &path_alias, &error) == 0);
      assert(path_alias);
      LinkerImageLease* soname_match = nullptr;
      assert(handles.OpenLocalSoname(handles.Anonymous(), "libicu.so", nullptr,
          &soname_match, &error) == 0);
      assert(soname_match);
      int path_same = 0;
      assert(darwin_art_linker_image_same(path_alias, soname_match, &path_same) == 0 && path_same);
      darwin_art_linker_image_release(path_alias);
      darwin_art_linker_image_release(soname_match);
      // Android checks a visible existing inode before new-file path access.
      // This child cannot admit an APEX path, but its direct libicu link grants
      // reuse of the already-loaded image. No new mapping/path grant is made.
      auto* restricted = handles.CreateChild(handles.Anonymous(), true, false, false,
          "/data/local/tmp", nullptr, nullptr, &error);
      assert(restricted);
      assert(handles.Link(restricted, handles.Exported("com_android_i18n"),
          "libicu.so", &error));
      LinkerImageLease* reused_path = nullptr;
      assert(handles.OpenLocalPath(restricted, "/apex/com.android.i18n/lib64/libicu.so",
          nullptr, &reused_path, &error) == 0);
      LinkerImageLease* visible_icu = nullptr;
      assert(handles.FindResident(restricted, "libicu.so", &visible_icu) == 0);
      assert(darwin_art_linker_image_same(reused_path, visible_icu, &path_same) == 0 && path_same);
      darwin_art_linker_image_release(reused_path);
      darwin_art_linker_image_release(visible_icu);
      assert(handles.OpenLocalPath(restricted, "/apex/com.android.i18n/lib64/libicuuc.so",
          nullptr, &reused_path, &error) < 0 && !reused_path);
      std::cerr << "resident inode before path admission: permitted link reused, unlisted image denied PASS\n";
      // A real image outside the selected i18n namespace must be denied even
      // when the guest filesystem can open the path; no lease is returned.
      LinkerImageLease* denied_path = nullptr;
      assert(handles.OpenLocalPath(handles.Exported("com_android_i18n"),
          "/apex/com.android.runtime/lib64/bionic/libc.so", nullptr,
          &denied_path, &error) < 0);
      assert(!denied_path);
      std::cerr << "real ICU path alias/SONAME identity and selected-namespace denial: PASS\n";
      // Deliberately stage another root from its admitted fd. Its dependency
      // graph must reuse already-published ICU mappings, not stage new copies.
      darwin_art::loader::DiscoveredGraph repeated;
      assert(handles.Discover(file, "libicu.so", &repeated, &error));
      const DarwinArtElfGraphSource* repeated_sources = nullptr;
      size_t repeated_count = 0;
      assert(darwin_art_elf_discovered_graph_sources(repeated.get(), &repeated_sources,
          &repeated_count, nullptr) == DARWIN_ART_ELF_OK);
      assert(repeated_count == 1);
      auto repeated_load = darwin_art::loader::LoadNamespaceGraph(std::move(repeated), nullptr, &error);
      if (!repeated_load) std::cerr << "resident ICU reuse failure: " << error << '\n';
      assert(repeated_load);
      DarwinArtElfSelectedImage* repeated_root = nullptr;
      assert(darwin_art_elf_select_image(repeated_load.get(), "libicu.so", &repeated_root, nullptr) == DARWIN_ART_ELF_OK);
      void* reused = nullptr;
      assert(darwin_art_elf_selected_native_dependency(repeated_root, "libicuuc.so", &reused, nullptr) == DARWIN_ART_ELF_OK);
      assert(handles.FindResident(handles.Exported("com_android_i18n"), "libicuuc.so", &published) == 0);
      assert(darwin_art_linker_image_same(static_cast<LinkerImageLease*>(reused), published, &same) == 0 && same);
      darwin_art_linker_image_release(published);
      darwin_art_elf_selected_image_release(repeated_root);
      std::cerr << "real ICU resident dependencies: original identity reused\n";
      void* original_libc = nullptr;
      assert(darwin_art_elf_selected_native_dependency(icu_image, "libc.so", &original_libc, nullptr) == DARWIN_ART_ELF_OK);
      const auto strlen_symbol = darwin_art::loader::ResolveBionicProviderImage(
          static_cast<LinkerImageLease*>(original_libc), "strlen", "LIBC");
      assert(strlen_symbol.status == DARWIN_ART_BIONIC_NAMESPACE_OK);
      assert(reinterpret_cast<size_t (*)(const char*)>(strlen_symbol.address)("original") == 8);
      darwin_art_elf_selected_image_release(icu_image);
      std::cerr << "real ICU original libc binding: retained and callable\n";
    }
    if (argc == 3 && std::string(argv[2]) == "--public-load") {
      LinkerImageLease* absent = nullptr;
      assert(handles.FindResident(handles.Anonymous(), "libicu.so", &absent) == 1 && !absent);
      android_dlextinfo request{};
      request.flags = ANDROID_DLEXT_USE_NAMESPACE;
      request.library_namespace = handles.Exported("com_android_i18n");
      // Admit the dependency in an earlier local group, so closing the later
      // wrapper must exercise external-group cascade rather than one graph.
      void* dependency_handle = android_dlopen_ext("libicuuc.so", 2, &request);
      assert(dependency_handle);
      request.library_namespace = handles.Anonymous();
      void* fresh = android_dlopen_ext("libicu.so", 2, &request);
      if (!fresh) std::cerr << darwin_art_linker_dlerror() << '\n';
      assert(fresh);
      assert(android_dlopen_ext("/apex/com.android.i18n/lib64/libicu.so", 2, &request) == fresh);
      assert(android_dlopen_ext("libicu.so", 2, &request) == fresh);
      assert(darwin_art_linker_dlclose(dependency_handle) == 0);
      LinkerImageLease* retained_dependency = nullptr;
      assert(handles.FindResident(handles.Exported("com_android_i18n"), "libicuuc.so", &retained_dependency) == 0);
      LinkerImageLease* root_image = nullptr;
      assert(handles.LibraryImage(reinterpret_cast<uintptr_t>(fresh), &root_image) == 0);
      DarwinArtLocalGroupMetadata dependency_group{}, root_group{};
      assert(darwin_art_linker_image_group_metadata(retained_dependency, &dependency_group) == 0);
      assert(darwin_art_linker_image_group_metadata(root_image, &root_group) == 0);
      assert(root_group.id != dependency_group.id);
      darwin_art_linker_image_release(root_image);
      darwin_art_linker_image_release(retained_dependency);
      std::cout << "public Android dlopen fresh ICU + path/SONAME same handle PASS\n";
    }
    const int old_fd = file.fd.get();
    assert(handles.OpenFile(handles.Anonymous(), "libdoesnotexist.so", nullptr, nullptr, &file) == 1);
    assert(file.fd.get() == -1 && !file.target && file.canonical_path.empty());
    errno = 0;
    assert(fcntl(old_fd, F_GETFD) == -1 && errno == EBADF);
    // Non-shared child must not inherit the root's APEX link.
    auto* isolated = handles.CreateChild(handles.Anonymous(), true, false, false,
        nullptr, "/data/app/lib", nullptr, &error);
    assert(isolated);
    assert(handles.OpenFile(isolated, "libicu.so", nullptr, nullptr, &file) == 1);
    auto* shared = handles.CreateChild(handles.Anonymous(), true, true, false,
        nullptr, "/data/app/lib", nullptr, &error);
    assert(shared && handles.OpenFile(shared, "libicu.so", nullptr, nullptr, &file) == 0);
    assert(file.target == handles.Exported("com_android_i18n"));
    assert(handles.ParentForCaller(nullptr, nullptr, &error) == handles.Anonymous());
    assert(handles.ParentForCaller(shared, nullptr, &error) == shared);
    assert(!handles.ParentForCaller(reinterpret_cast<android_namespace_t*>(uintptr_t{1}), nullptr, &error));
    if (argc == 3) {
      LinkerImageLease* caller = nullptr;
      assert(handles.FindResident(shared, "libicu.so", &caller) == 0);
      // Lookup through shared/link visibility must not change original owner.
      assert(handles.ParentForCaller(nullptr, caller, &error) == handles.Exported("com_android_i18n"));
      void* selected = nullptr;
      assert(darwin_art_linker_image_typed_payload(caller, DARWIN_ART_IMAGE_ELF_SELECTED, &selected) == 0);
      uintptr_t address = 0;
      assert(darwin_art_elf_selected_image_lookup(static_cast<DarwinArtElfSelectedImage*>(selected),
          "u_strlen", nullptr, &address, nullptr) == DARWIN_ART_ELF_OK && address);
      Dl_info registered_image{};
      assert(android::darwin_art_image_registry::darwin_art_android_elf_dladdr(
          reinterpret_cast<void*>(address), &registered_image) != 0);
      // This lookup is on the libicu.so wrapper, not libicuuc.so.
      assert(std::string(registered_image.dli_fname) == file.canonical_path);
      LinkerImageLease* resolved = nullptr;
      assert(handles.FindElfCaller(address, &resolved, &error) == 0);
      android_dlextinfo request{};
      request.flags = ANDROID_DLEXT_USE_NAMESPACE;
      request.library_namespace = handles.Exported("com_android_i18n");
      void* guest = android_dlopen_ext("libicu.so", 2, &request);
      assert(guest && android_dlopen_ext("libicu.so", 2, &request) == guest);
      assert(darwin_art_linker_dlsym(guest, "u_strlen") == reinterpret_cast<void*>(address));
      assert(!darwin_art_linker_dlsym(guest, "definitely_absent_test_symbol"));
      assert(darwin_art_linker_dlerror());
      LinkerImageLease* guest_image = nullptr;
      assert(handles.LibraryImage(reinterpret_cast<uintptr_t>(guest), &guest_image) == 0);
      int32_t guest_same = 0;
      assert(darwin_art_linker_image_same(caller, guest_image, &guest_same) == 0 && guest_same);
      const uint16_t sample[] = {'O', 'K', 0};
      assert(reinterpret_cast<int32_t(*)(const uint16_t*)>(address)(sample) == 2);
      darwin_art_linker_image_release(guest_image);
      request.library_namespace = nullptr;
      assert(!android_dlopen_ext("libicu.so", 2, &request));
      assert(darwin_art_linker_dlerror());
      request.flags = ANDROID_DLEXT_USE_LIBRARY_FD;
      assert(!android_dlopen_ext("libicu.so", 2, &request));
      assert(darwin_art_linker_dlerror());
      int32_t same = 0;
      assert(darwin_art_linker_image_same(caller, resolved, &same) == 0 && same);
      assert(handles.ParentForCaller(nullptr, resolved, &error) == handles.Exported("com_android_i18n"));
      auto* caller_child = __loader_android_create_namespace("icu-caller-child", nullptr, nullptr,
          ANDROID_NAMESPACE_TYPE_SHARED, nullptr, nullptr, reinterpret_cast<void*>(address));
      assert(caller_child);
      LinkerImageLease* inherited = nullptr;
      assert(handles.FindResident(caller_child, "libicu.so", &inherited) == 0);
      assert(darwin_art_linker_image_same(caller, inherited, &same) == 0 && same);
      darwin_art_linker_image_release(inherited);
      darwin_art_linker_image_release(resolved);
      assert(handles.FindElfCaller(1, &resolved, &error) == 1 && !resolved);
      assert(handles.ParentForCaller(isolated, caller, &error) == isolated);
      darwin_art_linker_image_release(caller);
      if (std::string(argv[2]) == "--public-load") {
        // Three opens in the fresh test and two here. All lookup leases are
        // released first; they are not additional Android dlopen references.
        for (int i = 0; i < 5; ++i) assert(darwin_art_linker_dlclose(guest) == 0);
        assert(darwin_art_linker_dlclose(guest) == -1);
        assert(darwin_art_linker_dlerror());
        LinkerImageLease* absent = nullptr;
        assert(!darwin_art_linker_dlsym(guest, "u_strlen"));
        assert(darwin_art_linker_dlerror());
        for (const char* name : {"libicu.so", "libicuuc.so"}) {
          assert(handles.FindResident(handles.Exported("com_android_i18n"), name, &absent) == 1 && !absent);
        }
        assert(android::darwin_art_image_registry::darwin_art_android_elf_dladdr(
            reinterpret_cast<void*>(address), &registered_image) == 0);
        request.flags = ANDROID_DLEXT_USE_NAMESPACE;
        request.library_namespace = handles.Exported("com_android_i18n");
        void* reloaded = android_dlopen_ext("libicu.so", 2, &request);
        assert(reloaded && reloaded != guest);
        assert(darwin_art_linker_dlclose(reloaded) == 0);
        assert(handles.FindResident(request.library_namespace, "libicu.so", &absent) == 1 && !absent);
        void* pinned = android_dlopen_ext("libicu.so", 0x1002, &request);
        assert(pinned);
        LinkerImageLease* pinned_image = nullptr;
        assert(handles.LibraryImage(reinterpret_cast<uintptr_t>(pinned), &pinned_image) == 0);
        uint8_t global_flag = 9, nodelete_flag = 0;
        assert(darwin_art_linker_image_group_retention_flags(pinned_image, &global_flag, &nodelete_flag) == 0);
        assert(global_flag == 0 && nodelete_flag == 1);
        darwin_art_linker_image_release(pinned_image);
        auto pinned_strlen = reinterpret_cast<int32_t(*)(const uint16_t*)>(darwin_art_linker_dlsym(pinned, "u_strlen"));
        assert(pinned_strlen && pinned_strlen(sample) == 2);
        assert(darwin_art_linker_dlclose(pinned) == 0);
        assert(pinned_strlen(sample) == 2);
        assert(android_dlopen_ext("libicu.so", 2, &request) == pinned);
        assert(darwin_art_linker_dlclose(pinned) == 0);
        std::cout << "public NODELETE fresh ICU survives last close and reuses original handle PASS\n";
        std::cout << "public close cascades to earlier ICU dependency group, removes image registration PASS\n";
      }
    }
  }
  auto* anonymous = android_create_namespace("anonymous-owner", nullptr, "/data/app/lib",
      ANDROID_NAMESPACE_TYPE_ISOLATED | ANDROID_NAMESPACE_TYPE_ALSO_USED_AS_ANONYMOUS,
      nullptr, process->Anonymous());
  assert(anonymous && process->Anonymous() == anonymous);
  assert(!android_create_namespace("second-anonymous", nullptr, nullptr,
      ANDROID_NAMESPACE_TYPE_ALSO_USED_AS_ANONYMOUS, nullptr, anonymous));
  assert(darwin_art_linker_dlerror() && process->Anonymous() == anonymous);
  auto* unknown_caller_child = android_create_namespace("unknown-caller", nullptr, nullptr,
      ANDROID_NAMESPACE_TYPE_SHARED, nullptr, nullptr);
  assert(unknown_caller_child);
  TestAnonymousInitialization(*process);
  assert(installation->Shutdown());
  assert(installation->Shutdown());
  assert(!darwin_art::loader::AcquireProcessNamespaces());
  assert(!android_link_namespaces(abi_child, nullptr, "libc.so"));
  assert(darwin_art_linker_dlerror());
  process.reset();
  loaded.reset();
  namespaces.reset();
  assert(darwin_art_bionic_fs_process_uninstall() == 0);
  assert(darwin_art_bionic_process_state_process_uninstall() == 0);
  assert(darwin_art_bionic_vm_process_uninstall() == 0);
  std::cout << "actual generated config -> guest VFS -> original bionic parser -> Rust namespaces PASS\n";
}
