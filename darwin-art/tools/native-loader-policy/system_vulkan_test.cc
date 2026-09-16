#include "linker_config.h"
#include "loader/configured_namespaces.h"
#include "loader/namespace_handles.h"
#include "loader/vulkan_image.h"
#include <cassert>
#include <cstdio>
extern "C" int darwin_art_bionic_vulkan_provider_ready();

void TestSystemVulkan() {
  using namespace darwin_art::loader;
  std::vector<std::unique_ptr<NamespaceConfig>> configs;
  auto config = std::make_unique<NamespaceConfig>("default");
  config->set_visible(true);
  configs.push_back(std::move(config));
  std::string error;
  auto configured = ConfiguredNamespaces::Create(configs, "", &error);
  assert(configured);
  auto* registry = configured->registry();
  const auto id = configured->Find("default");
  assert(!PublishVulkanImage(registry, id, "libEGL.so", "/system/lib64/libvulkan.so", &error));
  assert(!PublishVulkanImage(registry, id, "libvulkan.so", "relative", &error));
  uintptr_t address = 99;
  assert(ResolveVulkanImage(nullptr, "vkCreateInstance", nullptr, &address, &error) < 0 && address == 0);
  if (!darwin_art_bionic_vulkan_provider_ready()) {
    assert(!PublishVulkanImage(registry, id, "libvulkan.so", "/system/lib64/libvulkan.so", &error));
    std::puts("Vulkan image: absent real backend rejects publication PASS");
    return;
  }
  assert(PublishVulkanImage(registry, id, "libvulkan.so", "/system/lib64/libvulkan.so", &error));
  LinkerImageLease* retained = nullptr;
  {
    NamespaceHandles handles(std::move(configured));
    LinkerImageLease* image = nullptr;
    assert(handles.FindResident(handles.Exported("default"), "libvulkan.so", &image) == 0);
    assert(IsVulkanImage(image));
    retained = darwin_art_linker_image_clone(image);
    assert(retained);
    void* wrong = reinterpret_cast<void*>(1);
    assert(darwin_art_linker_image_typed_payload(image, DARWIN_ART_IMAGE_MACHO, &wrong) != 0 && !wrong);
    uintptr_t token = 0;
    assert(darwin_art_linker_handle_adopt(registry, &image, &token) == 0);
    address = handles.LibrarySymbol(token, "vkEnumerateInstanceVersion", &error);
    assert(address);
    uint32_t version = 0;
    assert(reinterpret_cast<int (*)(uint32_t*)>(address)(&version) == 0 && version != 0);
    assert(!handles.LibrarySymbol(token, "vkCreateMetalSurfaceEXT", &error));
    assert(!handles.LibrarySymbol(token, "vkCreateMacOSSurfaceMVK", &error));
    assert(!handles.LibrarySymbol(token, "malloc", &error));
    assert(!handles.LibrarySymbol(token, "vkNoSuchAndroidFunction", &error));
    assert(!handles.LibrarySymbol(token, "vkCreateInstance", &error, "LIBVULKAN"));
    assert(handles.CloseLibrary(token, &error) == 0);
  }
  assert(IsVulkanImage(retained));
  assert(ResolveVulkanImage(retained, "vkEnumerateInstanceVersion", nullptr, &address, &error) == 0);
  assert(darwin_art_bionic_vulkan_provider_ready() == 1);
  darwin_art_linker_image_release(retained);
  std::puts("Vulkan image: real version call, isolation, close and retained image lifetime PASS");
}
