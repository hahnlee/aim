#include "loader/namespace_handles.h"
#include <cassert>
#include <cstdio>
extern "C" void __loader_android_update_LD_LIBRARY_PATH(const char*);

void TestNamespaceSearchPaths(darwin_art::loader::NamespaceHandles& owner) {
  auto* system = owner.Exported("default");
  auto* i18n = owner.Exported("com_android_i18n");
  std::string defaults;
  assert(owner.DefaultLibraryPath(&defaults));
  darwin_art::loader::NamespaceFile initial;
  assert(owner.OpenFile(system, "libicuuc.so", nullptr, nullptr, &initial) == 0);
  assert(initial.target == i18n);
  __loader_android_update_LD_LIBRARY_PATH(
      "/missing-loader-path:/system/lib64/libz.so:/apex/com.android.i18n/lib64");
  darwin_art::loader::NamespaceFile changed;
  assert(owner.OpenFile(system, "libicuuc.so", nullptr, nullptr, &changed) == 0);
  assert(changed.target == system && changed.canonical_path == initial.canonical_path);
  std::string after;
  assert(owner.DefaultLibraryPath(&after) && after == defaults);
  __loader_android_update_LD_LIBRARY_PATH(nullptr);
  darwin_art::loader::NamespaceFile restored;
  assert(owner.OpenFile(system, "libicuuc.so", nullptr, nullptr, &restored) == 0);
  assert(restored.target == i18n);
  std::puts("AOSP search path update: guest resolution, local-before-link admission, defaults preserved, null clear PASS");
}
