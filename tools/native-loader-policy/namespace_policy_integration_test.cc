#include "loader/android_dlext_types.h"
#include "loader/namespace_handles.h"
#include "native_loader_namespace.h"
#include "loader/bionic_provider_image.h"
#include <cassert>
#include <cstdio>
extern "C" int aim_linker_dlclose(void*);
extern "C" void* aim_linker_dlsym(void*, const char*);

void TestOriginalNamespacePolicy(aim::loader::NamespaceHandles& owner) {
  auto parent = android::NativeLoaderNamespace::GetExportedNamespace("com_android_i18n", false);
  assert(parent.ok() && !parent->IsBridged());
  auto child = android::NativeLoaderNamespace::Create("original-policy", "/data/app/lib", "",
      &*parent, true, false, false);
  assert(child.ok() && !child->IsBridged());
  assert(owner.Name(child->ToRawAndroidNamespace()) == "original-policy");
  assert(child->Link(nullptr, "libc.so").ok());
  auto opened = child->Load("libc.so");
  assert(opened.ok() && *opened);
  auto repeated = child->Load("libc.so");
  assert(repeated.ok() && *repeated == *opened);
  LinkerImageLease* library = nullptr;
  assert(owner.LibraryImage(reinterpret_cast<uintptr_t>(*opened), &library) == 0);
  auto symbol = aim::loader::ResolveBionicProviderImage(library, "strlen", "LIBC");
  assert(symbol.status == AIM_BIONIC_NAMESPACE_OK);
  assert(aim_linker_dlsym(*opened, "strlen") == reinterpret_cast<void*>(symbol.address));
  assert(reinterpret_cast<size_t(*)(const char*)>(symbol.address)("loaded") == 6);
  aim_linker_image_release(library);
  assert(aim_linker_dlclose(*opened) == 0);
  assert(aim_linker_dlclose(*repeated) == 0);
  assert(aim_linker_dlclose(*opened) == -1);
  assert(aim_linker_dlerror());
  assert(!aim_linker_dlsym(reinterpret_cast<void*>(uintptr_t{2}), "strlen"));
  assert(aim_linker_dlerror());
  auto missing = child->Load("libmissing.so");
  assert(!missing.ok());
  assert(missing.error().message().find("library not found") != std::string::npos);
  LinkerImageLease* libc = nullptr;
  assert(owner.FindResident(child->ToRawAndroidNamespace(), "libc.so", &libc) == 0);
  aim_linker_image_release(libc);
  auto unsupported = android::NativeLoaderNamespace::Create("exempt", "", "", &*parent,
      false, true, false);
  assert(!unsupported.ok());
  assert(unsupported.error().message().find("failed to create native namespace name:exempt") != std::string::npos);
  const char* detail = aim_linker_dlerror();
  assert(detail && std::string(detail).find("unsupported linker policy") != std::string::npos);
  std::puts("original NativeLoaderNamespace -> actual process namespace create/link/errors PASS");
}
