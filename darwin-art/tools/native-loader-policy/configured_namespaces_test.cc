#include "loader/configured_namespaces.h"
#include "loader/namespace_handles.h"
#include "linker_config.h"
#include <cassert>
#include <iostream>

int main() {
  std::vector<std::unique_ptr<NamespaceConfig>> configs;
  auto root = std::make_unique<NamespaceConfig>("default");
  root->set_search_paths({"/system/lib64"});
  root->set_visible(true);
  root->add_namespace_link("module", "libactual.so", false);
  configs.push_back(std::move(root));
  auto module = std::make_unique<NamespaceConfig>("module");
  module->set_isolated(true);
  module->set_search_paths({"/apex/module/lib64"});
  module->set_allowed_libs({"libactual.so"});
  configs.push_back(std::move(module));
  std::string error;
  auto owner = darwin_art::loader::ConfiguredNamespaces::Create(configs, "/data/local/lib", &error);
  assert(owner && error.empty() && owner->Find("default") && owner->Find("module"));
  uint64_t visible = 0;
  assert(darwin_art_linker_namespace_exported(owner->registry(), "default", &visible) == 0);
  assert(visible == owner->Find("default"));
  assert(darwin_art_linker_namespace_exported(owner->registry(), "module", &visible) == 1);
  // Real registry image leases; this stack token is not an executable DSO.
  int image_token = 7;
  auto retain = +[](void* value) -> void* { return value; };
  auto release = +[](void*) {};
  assert(darwin_art_linker_namespace_publish(owner->registry(), owner->Find("module"),
      "libactual.so", "/apex/module/lib64/libactual.so", &image_token, retain, release) == 0);
  assert(darwin_art_linker_namespace_publish(owner->registry(), owner->Find("module"),
      "libactual.so", "/elsewhere/libactual.so", &image_token, retain, release) != 0);
  LinkerImageLease* lease = nullptr;
  assert(darwin_art_linker_namespace_find(owner->registry(), owner->Find("default"),
      "libactual.so", &lease) == 0);
  assert(darwin_art_linker_image_payload(lease) == &image_token);
  darwin_art_linker_image_release(lease);
  assert(darwin_art_linker_namespace_find(owner->registry(), owner->Find("default"),
      "libunlinked.so", &lease) == 1 && lease == nullptr);
  configs[1]->add_namespace_link("default", "", true);
  assert(darwin_art::loader::ConfiguredNamespaces::Create(configs, "", &error));
  configs[1]->add_namespace_link("absent", "libmissing.so", false);
  assert(!darwin_art::loader::ConfiguredNamespaces::Create(configs, "", &error));
  assert(error.find("absent") != std::string::npos);
  assert(owner->Find("default"));  // Failed staging did not mutate the live owner.
  // Opaque test resources with mapped-image visibility metadata, not DSOs.
  int df_global = 8, rt_global = 9;
  assert(darwin_art_linker_namespace_publish_flags(owner->registry(), owner->Find("default"),
      "libdf.so", "/system/lib64/libdf.so", &df_global, retain, release, 2, 0) == 0);
  assert(darwin_art_linker_namespace_publish_flags(owner->registry(), owner->Find("default"),
      "librt.so", "/system/lib64/librt.so", &rt_global, retain, release, 0, 1) == 0);
  darwin_art::loader::NamespaceHandles handles(std::move(owner));
  auto* root_handle = handles.Exported("default");
  assert(root_handle && root_handle == handles.Exported("default"));
  assert(handles.Anonymous() == root_handle);
  auto* child = handles.CreateChild(root_handle, true, false, true,
      nullptr, "/data/app/lib", "/data/app", &error);
  assert(child && error.empty() && handles.Anonymous() == child);
  assert(handles.FindResident(child, "libdf.so", &lease) == 0);
  assert(darwin_art_linker_image_payload(lease) == &df_global);
  darwin_art_linker_image_release(lease);
  assert(handles.FindResident(child, "librt.so", &lease) == 1 && !lease);
  // Root links are not inherited by a non-shared child.
  assert(handles.FindResident(child, "libactual.so", &lease) == 1 && !lease);
  auto* shared_root = handles.CreateChild(root_handle, true, true, false,
      nullptr, "/data/app/lib", nullptr, &error);
  assert(shared_root);
  assert(handles.FindResident(shared_root, "librt.so", &lease) == 0);
  assert(darwin_art_linker_image_payload(lease) == &rt_global);
  darwin_art_linker_image_release(lease);
  assert(handles.FindResident(shared_root, "libactual.so", &lease) == 0);
  darwin_art_linker_image_release(lease);
  assert(!handles.CreateChild(root_handle, true, false, true,
      nullptr, "/data/app/lib", "/data/app", &error));
  assert(handles.Anonymous() == child);
  auto* shared = handles.CreateChild(child, true, true, false,
      nullptr, "/data/other/lib", "/data/other", &error);
  assert(shared && shared != child && handles.Anonymous() == child);
  assert(!handles.CreateChild(nullptr, true, false, false,
      nullptr, nullptr, nullptr, &error));
  assert(!handles.Exported(nullptr) && !handles.Exported("module"));
  assert(handles.Link(root_handle, nullptr, "libactual.so", &error) && error.empty());
  assert(!handles.Link(nullptr, root_handle, "libactual.so", &error));
  assert(!handles.Link(root_handle, nullptr, "", &error));
  // A valid handle from another owner is not valid in this registry.
  std::vector<std::unique_ptr<NamespaceConfig>> foreign_configs;
  auto foreign_root = std::make_unique<NamespaceConfig>("default");
  foreign_root->set_visible(true);
  foreign_configs.push_back(std::move(foreign_root));
  darwin_art::loader::NamespaceHandles foreign(
      darwin_art::loader::ConfiguredNamespaces::Create(foreign_configs, "", &error));
  assert(!handles.Link(root_handle, foreign.Exported("default"), "libactual.so", &error));
  assert(!foreign.Link(root_handle, nullptr, "libactual.so", &error));
  assert(foreign.FindResident(root_handle, "libdf.so", &lease) < 0 && !lease);
  assert(!foreign.CreateChild(root_handle, true, false, false,
      nullptr, nullptr, nullptr, &error));
  assert(!foreign.CreateChild(foreign.Anonymous(), true, false, true,
      nullptr, "relative/path", nullptr, &error));
  // A failed creation must not consume the one-time anonymous assignment.
  assert(foreign.CreateChild(foreign.Anonymous(), true, false, true,
      nullptr, "/data/app/lib", nullptr, &error));
  configs.clear();
  assert(!darwin_art::loader::ConfiguredNamespaces::Create(configs, "", &error));
  std::cout << "original NamespaceConfig -> Rust registry creation/link/export/error PASS\n";
}
