#include "loader/namespace_handles.h"
#include <cassert>
#include <cstdio>
extern "C" bool __loader_android_init_anonymous_namespace(const char*, const char*);

void TestAnonymousInitialization(darwin_art::loader::NamespaceHandles& owner) {
  auto* before = owner.Anonymous();
  assert(__loader_android_init_anonymous_namespace("libz.so", "/apex/com.android.i18n/lib64"));
  auto* first = owner.Anonymous();
  assert(first && first != before && owner.Name(first) == "(anonymous)");
  darwin_art::loader::NamespaceFile local, linked, denied;
  assert(owner.OpenFile(first, "libicuuc.so", nullptr, nullptr, &local) == 0);
  assert(local.target == first);
  assert(owner.OpenFile(first, "libz.so", nullptr, nullptr, &linked) == 0);
  assert(linked.target == owner.Exported("default"));
  assert(owner.OpenFile(first, "libbase.so", nullptr, nullptr, &denied) == 1);
  assert(__loader_android_init_anonymous_namespace("libbase.so", nullptr));
  auto* second = owner.Anonymous();
  assert(second && second != first);
  assert(owner.OpenFile(second, "libbase.so", nullptr, nullptr, &linked) == 0);
  assert(owner.OpenFile(second, "libz.so", nullptr, nullptr, &denied) == 1);
  assert(owner.OpenFile(first, "libz.so", nullptr, nullptr, &linked) == 0);
  assert(!__loader_android_init_anonymous_namespace(nullptr, nullptr));
  assert(darwin_art_linker_dlerror());
  assert(owner.Anonymous() != second); // Matches original failure ordering.
  std::puts("anonymous init: isolated paths, exact links, repeated init, old identity, failed-link ordering PASS");
}
