#include "loader/bionic_provider_set.h"
#include "loader/bionic_symbol_lookup.h"
#include "loader/bionic_provider_image.h"
#include "loader/namespace_admission.h"
#include "aim_guest_image.h"
static int* (*guest_errno)();
static int ReadGuestErrno() { return *guest_errno(); }
#include <cassert>
#include <iostream>

int main() {
  std::string error;
  aim::loader::BionicProviderSet unsealed(aim_bionic_namespace_create());
  assert(aim_bionic_namespace_image_status(unsealed.get(), "libc.so") ==
      AIM_BIONIC_NAMESPACE_NOT_SEALED);
  unsealed.reset();
  auto owner = aim::loader::CreateBionicProviderSet(&error);
  assert(owner && error.empty());
  const auto default_strlen = aim_bionic_namespace_resolve(owner.get(), "libc.so", "strlen", nullptr);
  assert(default_strlen.status == AIM_BIONIC_NAMESPACE_OK && default_strlen.address);
  assert(reinterpret_cast<size_t (*)(const char*)>(default_strlen.address)("default") == 7);
  const auto default_cos = aim_bionic_namespace_resolve(owner.get(), "libm.so", "cos", nullptr);
  assert(default_cos.status == AIM_BIONIC_NAMESPACE_OK && default_cos.address);
  assert(reinterpret_cast<double (*)(double)>(default_cos.address)(0.0) == 1.0);
  const auto default_dlopen = aim_bionic_namespace_resolve(owner.get(), "libdl.so", "dlopen", nullptr);
  assert(default_dlopen.status == AIM_BIONIC_NAMESPACE_OK && default_dlopen.address);
  assert(default_dlopen.address == aim_bionic_namespace_resolve(owner.get(), "libdl.so", "dlopen", "LIBC").address);
  assert(aim_bionic_namespace_resolve(owner.get(), "libc.so", "mallopt", "INVALID").status == AIM_BIONIC_NAMESPACE_UNKNOWN_VERSION);
  assert(aim_bionic_namespace_resolve(owner.get(), "libc.so", "mallopt", "LIBC_O").status == AIM_BIONIC_NAMESPACE_OK);
  assert(aim_bionic_namespace_resolve(owner.get(), "libc.so", "mallopt", nullptr).status == AIM_BIONIC_NAMESPACE_OK);
  const char* repeated[] = {"libaaudio.so", "libaaudio.so"};
  auto ordered = aim::loader::LookupBionicDependencies(owner.get(), repeated, 2, "AAudioStreamBuilder_delete");
  assert(ordered.status == AIM_BIONIC_NAMESPACE_OK && ordered.address);
  ordered = aim::loader::LookupBionicDependencies(owner.get(), repeated, 2, "not_exported");
  assert(aim::loader::IsBionicSymbolMiss(ordered.status) && !ordered.address);
  assert(!aim::loader::IsBionicSymbolMiss(AIM_BIONIC_NAMESPACE_SHUTTING_DOWN));
  assert(!aim::loader::IsBionicSymbolMiss(AIM_BIONIC_NAMESPACE_PROVIDER_REJECTED));
  assert(!aim::loader::IsBionicSymbolMiss(AIM_BIONIC_NAMESPACE_NOT_SEALED));
  assert(aim_bionic_namespace_seal(owner.get()) == AIM_BIONIC_NAMESPACE_ALREADY_SEALED);
  auto symbol = aim_bionic_namespace_resolve(owner.get(), "libc.so", "strlen", "LIBC");
  assert(symbol.status == AIM_BIONIC_NAMESPACE_OK && symbol.address);
  auto length = reinterpret_cast<size_t (*)(const char*)>(symbol.address);
  assert(length("Android owned") == 13);
  assert(aim_bionic_namespace_resolve(owner.get(), "libc.so", "strlen", "INVALID").address == 0);
  auto transferred = std::move(owner);
  assert(!owner && transferred);
  aim::loader::SharedBionicProviders shared(std::move(transferred));
  auto errno_symbol = aim_bionic_namespace_resolve(shared.get(), "libc.so", "__errno", "LIBC");
  assert(errno_symbol.status == AIM_BIONIC_NAMESPACE_OK && errno_symbol.address);
  guest_errno = reinterpret_cast<int* (*)()>(errno_symbol.address);
  auto* registry = aim_linker_registry_create();
  uint64_t id = 0;
  assert(aim_linker_namespace_create(registry, 1, "", "/system/lib64", "", 0, &id) == 0);
  assert(aim::loader::PublishBionicProviderImage(registry, id, "libc.so",
      "/system/lib64/libc.so", shared, &error));
  assert(!aim::loader::PublishBionicProviderImage(registry, id, "unowned.so",
      "/system/lib64/unowned.so", shared, &error));
  LinkerImageLease* lease = nullptr;
  auto* discovery = aim_linker_discovery_create(registry, id,
      "/system/lib64/libclient.so", aim_fs_open_native_directory,
      aim_fs_open_native_image, ReadGuestErrno);
  assert(discovery);
  aim::loader::NamespaceAdmission admission{discovery};
  AimElfAdmission first{}, second{};
  assert(aim::loader::AdmitNamespaceDependency(&admission, 1, "client.so", "libc.so", nullptr, &first) == 0);
  assert(aim::loader::AdmitNamespaceDependency(&admission, 1, "client.so", "libc.so", nullptr, &second) == 0);
  assert(first.kind == 2 && first.fd == -1 && first.resident && first.release);
  assert(first.image == second.image && first.image > 1);
  first.release(first.resident);
  second.release(second.resident);
  AimElfAdmission rejected{99, 99, 99, nullptr, nullptr};
  assert(aim::loader::AdmitNamespaceDependency(&admission, 0,
      "client.so", "libc.so", nullptr, &rejected) < 0);
  assert(rejected.kind == 0 && rejected.fd == -1 && rejected.image == 0 &&
      !rejected.resident && !rejected.release);
  assert(aim_linker_discovery_find_resident(discovery, 0, "libc.so", &lease) < 0 && !lease);
  assert(aim_linker_discovery_find_resident(discovery, 1, "missing.so", &lease) == 1 && !lease);
  assert(aim_linker_discovery_resident_image(discovery, 0, &lease) < 0 && !lease);
  assert(aim_linker_discovery_resident_image(discovery, UINT64_MAX, &lease) < 0 && !lease);
  assert(aim_linker_discovery_resident_image(discovery, 1, &lease) == 1 && !lease);
  assert(aim_linker_discovery_resident_image(discovery, first.image, &lease) == 0);
  aim_linker_discovery_destroy(discovery);
  uint64_t isolated = 0;
  assert(aim_linker_namespace_create(registry, 1, "", "/data/app", "", 0, &isolated) == 0);
  auto* child = aim_linker_discovery_create(registry, isolated,
      "/data/app/client.so", aim_fs_open_native_directory,
      aim_fs_open_native_image, ReadGuestErrno);
  assert(child);
  aim::loader::NamespaceAdmission child_admission{child};
  assert(aim::loader::AdmitNamespaceDependency(&child_admission, 1,
      "client.so", "libc.so", nullptr, &rejected) < 0);
  assert(rejected.kind == 0 && rejected.fd == -1 && !rejected.resident);
  LinkerImageLease* linked = nullptr;
  assert(aim_linker_discovery_find_resident(child, 1, "libc.so", &linked) == 1 && !linked);
  assert(aim_linker_namespace_link(registry, isolated, id, "libc.so") == 0);
  assert(aim::loader::AdmitNamespaceDependency(&child_admission, 1,
      "client.so", "libc.so", nullptr, &rejected) == 0);
  assert(rejected.kind == 2 && rejected.resident && rejected.release);
  rejected.release(rejected.resident);
  // A link granting libc does not grant libdl to the app. Lookup originating
  // from the retained libc image still belongs to libc's primary namespace.
  assert(aim::loader::PublishBionicProviderImage(registry, id, "libdl.so",
      "/system/lib64/libdl.so", shared, &error));
  assert(aim_linker_discovery_find_resident(child, 1, "libdl.so", &linked) == 1 && !linked);
  assert(aim_linker_discovery_find_resident(child, rejected.image, "libdl.so", &linked) == 0);
  aim_linker_image_release(linked);
  linked = nullptr;
  assert(aim_linker_discovery_find_resident(child, 1, "libc.so", &linked) == 0);
  aim_linker_image_release(linked);
  aim_linker_discovery_destroy(child);
  shared.reset();
  aim_linker_registry_destroy(registry);
  symbol = aim::loader::ResolveBionicProviderImage(lease, "strlen", "LIBC");
  assert(symbol.status == AIM_BIONIC_NAMESPACE_OK);
  assert(reinterpret_cast<size_t (*)(const char*)>(symbol.address)("retained") == 8);
  void* wrong = nullptr;
  assert(aim_linker_image_typed_payload(lease, AIM_IMAGE_ELF_SELECTED, &wrong) == -4);
  aim_linker_image_release(lease);
  std::cout << "real Bionic provider create/seal/resolve/call/owned teardown PASS\n";
}
