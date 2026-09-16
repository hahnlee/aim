#include "loader/namespace_handles.h"
#include "loader/namespace_operation.h"
#include "loader/namespace_loader_abi.h"
#include "loader/android_dlext_types.h"
#include <cassert>
#include <chrono>
#include <cstdio>
#include <future>
#include <thread>

extern "C" int darwin_art_fdsan_get_error_level();
extern "C" int darwin_art_fdsan_set_error_level(int);
extern "C" const void* darwin_art_bionic___system_property_find(const char*);

void TestTargetSdkPolicy() {
  using namespace darwin_art::loader;
  // This fixture's installed property snapshot has no debug.fdsan override.
  assert(!darwin_art_bionic___system_property_find("debug.fdsan"));
  const int saved_fdsan = darwin_art_fdsan_get_error_level();
  std::string error;
  auto configured = ConfiguredNamespaces::Read("/linkerconfig/ld.config.txt",
      "/system/bin/app_process64", false, false, "", &error);
  assert(configured && error.empty());
  auto* registry = configured->registry();
  const int initial_sdk = configured->target_sdk_version();
  NamespaceHandles owner(std::move(configured));
  assert(owner.TargetSdkVersion() == initial_sdk);
  assert(darwin_art_fdsan_get_error_level() == saved_fdsan);

  darwin_art_fdsan_set_error_level(3);
  assert(owner.SetTargetSdkVersion(29));
  assert(owner.TargetSdkVersion() == 29);
  assert(darwin_art_fdsan_get_error_level() == 1);
  // Moving to30 does not undo an earlier fdsan policy update in AOSP.
  assert(owner.SetTargetSdkVersion(30));
  assert(darwin_art_fdsan_get_error_level() == 1);
  assert(owner.SetTargetSdkVersion(0));
  assert(owner.TargetSdkVersion() == __ANDROID_API__);

  std::promise<void> started, finished;
  auto starting = started.get_future();
  auto finishing = finished.get_future();
  std::thread competing;
  {
    NamespaceOperation operation(registry);
    assert(operation);
    assert(owner.SetTargetSdkVersion(31)); // Same-thread reentry is legal.
    competing = std::thread([&] {
      started.set_value();
      assert(owner.SetTargetSdkVersion(32));
      finished.set_value();
    });
    starting.wait();
    assert(finishing.wait_for(std::chrono::milliseconds(20)) == std::future_status::timeout);
    assert(owner.TargetSdkVersion() == 31);
  }
  assert(finishing.wait_for(std::chrono::seconds(5)) == std::future_status::ready);
  competing.join();
  assert(owner.TargetSdkVersion() == 32);
  darwin_art_fdsan_set_error_level(saved_fdsan);
  std::puts("Target SDK owner: config, zero normalization, fdsan, recursive operation serialization PASS");
}

void TestImageSdkSnapshot(darwin_art::loader::NamespaceHandles& owner) {
  const int initial_sdk = owner.TargetSdkVersion();
  assert(__loader_android_get_application_target_sdk_version() == initial_sdk);
  __loader_android_set_application_target_sdk_version(34);
  assert(owner.TargetSdkVersion() == 34);
  assert(owner.SetTargetSdkVersion(35));
  assert(__loader_android_get_application_target_sdk_version() == 35);
  __loader_android_set_application_target_sdk_version(0);
  assert(owner.TargetSdkVersion() == __ANDROID_API__);
  assert(owner.SetTargetSdkVersion(initial_sdk));
  std::string error;
  assert(owner.SetTargetSdkVersion(31));
  const auto first = owner.OpenLibrary(0, "libsync.so", 2, 0, nullptr, &error);
  assert(first && error.empty());
  LinkerImageLease* image = nullptr;
  assert(owner.LibraryImage(first, &image) == 0);
  int32_t sdk = 0;
  assert(darwin_art_linker_image_target_sdk(image, &sdk) == 0 && sdk == 31);
  uint8_t eligible = 99;
  assert(darwin_art_linker_image_linear_lookup_eligible(image, &eligible) == 0 && eligible == 0);
  assert(owner.SetTargetSdkVersion(32));
  const auto second = owner.OpenLibrary(0, "libsync.so", 2, 0, nullptr, &error);
  assert(second && error.empty());
  LinkerImageLease* reused = nullptr;
  assert(owner.LibraryImage(second, &reused) == 0);
  int32_t same = 0;
  assert(darwin_art_linker_image_same(image, reused, &same) == 0 && same == 1);
  assert(darwin_art_linker_image_target_sdk(reused, &sdk) == 0 && sdk == 31);
  assert(owner.CloseLibrary(first, &error) == 0);
  assert(owner.CloseLibrary(second, &error) == 0);
  assert(darwin_art_linker_image_target_sdk(image, &sdk) == 0 && sdk == 31);
  darwin_art_linker_image_release(reused);
  darwin_art_linker_image_release(image);
  assert(owner.SetTargetSdkVersion(initial_sdk));
  std::puts("Image SDK snapshot: real ELF publication, resident reuse, retained metadata PASS");
}

void TestLinearSymbolPhase(darwin_art::loader::NamespaceHandles& owner) {
  const int sdk = owner.TargetSdkVersion();
  const int fdsan = darwin_art_fdsan_get_error_level();
  std::string error;
  auto* child = owner.CreateChild(owner.Anonymous(), true, false, false,
      nullptr, "/system/lib64", "/system/lib64", &error, "linear-symbol-test");
  assert(child && error.empty());
  assert(owner.Link(child, owner.Anonymous(), "libc.so:libm.so:libdl.so:libc++.so", &error));
  assert(owner.SetTargetSdkVersion(22));
  auto handle = owner.OpenLibrary(0, "libsync.so", 2, ANDROID_DLEXT_USE_NAMESPACE, child, &error);
  assert(handle && error.empty());
  LinkerImageLease* image = nullptr;
  assert(owner.LibraryImage(handle, &image) == 0);
  uintptr_t symbol = 0;
  assert(owner.LinearSymbol(child, nullptr, "sync_wait", &symbol, &error) == 0);
  assert(symbol == owner.LibrarySymbol(handle, "sync_wait", &error));
  assert(reinterpret_cast<int (*)(int, int)>(symbol)(-1, 0) == -1);
  assert(owner.SetTargetSdkVersion(36));
  assert(owner.LinearSymbol(child, nullptr, "sync_wait", &symbol, &error) == 0);
  // NEXT begins after the exact image even when it remains SDK22 eligible.
  assert(owner.LinearSymbol(child, image, "sync_wait", &symbol, &error) == 1);
  assert(symbol == 0 && error.empty());
  assert(owner.CloseLibrary(handle, &error) == 0);
  darwin_art_linker_image_release(image);
  assert(owner.SetTargetSdkVersion(sdk));
  darwin_art_fdsan_set_error_level(fdsan);
  std::puts("Linear symbol phase: namespace order, linked SDK, NEXT skip, actual ELF call PASS");
}
