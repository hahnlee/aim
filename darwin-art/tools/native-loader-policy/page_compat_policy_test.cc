#include "loader/namespace_handles.h"
#include "loader/namespace_loader_abi.h"
#include <cassert>
#include <cstdio>

extern "C" const void* darwin_art_bionic___system_property_find(const char*);

void TestPageCompatPolicy(darwin_art::loader::NamespaceHandles& owner) {
  // This installed test snapshot intentionally has no developer override.
  assert(!darwin_art_bionic___system_property_find("bionic.linker.16kb.app_compat.enabled"));
  const bool saved = owner.Effective16KbAppCompatMode();
  __loader_android_set_16kb_appcompat_mode(true);
  assert(owner.Effective16KbAppCompatMode());
  __loader_android_set_16kb_appcompat_mode(false);
  assert(!owner.Effective16KbAppCompatMode());
  assert(owner.Set16KbAppCompatMode(saved));
  std::puts("Android page compatibility: public setter uses process linker owner PASS");
}
