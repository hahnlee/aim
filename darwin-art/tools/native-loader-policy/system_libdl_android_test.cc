#include "loader/namespace_handles.h"
#include "loader/namespace_loader_abi.h"
#include "loader/android_dlext_types.h"
#include <cassert>
#include <cstdio>
#include <cstring>

void TestSystemLibdlAndroid(darwin_art::loader::NamespaceHandles& owner) {
  std::string error;
  auto* ns = owner.Exported("default");
  const auto handle = owner.OpenLibrary(0,
      "/apex/com.android.runtime/lib64/bionic/libdl_android.so", 2,
      ANDROID_DLEXT_USE_NAMESPACE, ns, &error);
  if (!handle) std::fprintf(stderr, "original libdl_android: %s\n", error.c_str());
  assert(handle && error.empty());
  auto symbol = [&](const char* name) {
    auto address = owner.LibrarySymbol(handle, name, &error);
    assert(address && error.empty());
    return address;
  };
  const int saved_sdk = owner.TargetSdkVersion();
  auto set_sdk = reinterpret_cast<void (*)(int)>(symbol("android_set_application_target_sdk_version"));
  set_sdk(33);
  assert(owner.TargetSdkVersion() == 33);
  set_sdk(saved_sdk);
  const bool saved_compat = owner.Effective16KbAppCompatMode();
  auto set_compat = reinterpret_cast<void (*)(bool)>(symbol("android_set_16kb_appcompat_mode"));
  set_compat(true);
  assert(owner.Effective16KbAppCompatMode());
  set_compat(saved_compat);
  auto exported = reinterpret_cast<android_namespace_t* (*)(const char*)>(
      symbol("android_get_exported_namespace"));
  assert(exported("default") == ns);
  auto create = reinterpret_cast<android_namespace_t* (*)(const char*, const char*,
      const char*, uint64_t, const char*, android_namespace_t*)>(symbol("android_create_namespace"));
  auto* child = create("original-libdl-child", nullptr, "/system/lib64", 1,
      "/system/lib64", ns);
  assert(child && child != ns);
  assert(!owner.OpenLibrary(0, "ld-android.so", 2, ANDROID_DLEXT_USE_NAMESPACE, child, &error));
  auto link = reinterpret_cast<bool (*)(android_namespace_t*, android_namespace_t*, const char*)>(
      symbol("android_link_namespaces"));
  assert(link(child, ns, "ld-android.so"));
  const auto linked_loader = owner.OpenLibrary(0, "ld-android.so", 2,
      ANDROID_DLEXT_USE_NAMESPACE, child, &error);
  assert(linked_loader && error.empty());
  assert(owner.LibrarySymbol(linked_loader, "__loader_android_get_application_target_sdk_version",
      &error) == reinterpret_cast<uintptr_t>(&__loader_android_get_application_target_sdk_version));
  assert(__loader_dlclose(reinterpret_cast<void*>(linked_loader)) == 0);
  // All original wrappers retain their public ELF symbols; no replacement APK
  // or rewritten libdl_android is involved.
  for (const char* name : {"android_get_LD_LIBRARY_PATH", "android_update_LD_LIBRARY_PATH",
      "android_init_anonymous_namespace", "android_dlwarning"}) (void)symbol(name);
  auto again = owner.OpenLibrary(0,
      "/apex/com.android.runtime/lib64/bionic/libdl_android.so", 2,
      ANDROID_DLEXT_USE_NAMESPACE, ns, &error);
  assert(again && error.empty());
  assert(owner.LibrarySymbol(again, "android_set_application_target_sdk_version", &error)
      == reinterpret_cast<uintptr_t>(set_sdk));
  assert(__loader_dlclose(reinterpret_cast<void*>(again)) == 0);
  assert(__loader_dlclose(reinterpret_cast<void*>(handle)) == 0);
  std::puts("original libdl_android: admitted ld-android, SDK/page policy, caller namespace, reuse PASS");
}
