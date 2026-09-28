#include <vulkan/vulkan.h>

#include <algorithm>
#include <cstdio>
#include <cstring>
#include <vector>

int main() {
  uint32_t instance_version = 0;
  if (vkEnumerateInstanceVersion(&instance_version) != VK_SUCCESS) return 2;

  const VkApplicationInfo application_info{
      VK_STRUCTURE_TYPE_APPLICATION_INFO, nullptr, "aim", 1,
      "aim", 1, VK_API_VERSION_1_1};
  const VkInstanceCreateInfo instance_info{
      VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, nullptr, 0, &application_info,
      0, nullptr, 0, nullptr};
  VkInstance instance = VK_NULL_HANDLE;
  if (vkCreateInstance(&instance_info, nullptr, &instance) != VK_SUCCESS) return 3;

  uint32_t physical_device_count = 0;
  if (vkEnumeratePhysicalDevices(instance, &physical_device_count, nullptr) !=
          VK_SUCCESS ||
      physical_device_count == 0) {
    vkDestroyInstance(instance, nullptr);
    return 4;
  }
  std::vector<VkPhysicalDevice> devices(physical_device_count);
  if (vkEnumeratePhysicalDevices(instance, &physical_device_count,
                                 devices.data()) != VK_SUCCESS) {
    vkDestroyInstance(instance, nullptr);
    return 5;
  }

  VkPhysicalDeviceSamplerYcbcrConversionFeatures ycbcr{
      VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SAMPLER_YCBCR_CONVERSION_FEATURES};
  VkPhysicalDeviceFeatures2 features{VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2};
  features.pNext = &ycbcr;
  vkGetPhysicalDeviceFeatures2(devices.front(), &features);

  uint32_t extension_count = 0;
  vkEnumerateDeviceExtensionProperties(devices.front(), nullptr,
                                       &extension_count, nullptr);
  std::vector<VkExtensionProperties> extensions(extension_count);
  vkEnumerateDeviceExtensionProperties(devices.front(), nullptr,
                                       &extension_count, extensions.data());
  const auto has_extension = [&](const char* name) {
    return std::ranges::any_of(extensions, [&](const auto& extension) {
      return std::strcmp(extension.extensionName, name) == 0;
    });
  };

  VkPhysicalDeviceProperties properties{};
  vkGetPhysicalDeviceProperties(devices.front(), &properties);
  std::printf(
      "moltenvk-capability: api=%u.%u.%u device=%s ycbcr=%u "
      "external-memory-metal=%u android-ahb=%u\n",
      VK_API_VERSION_MAJOR(instance_version),
      VK_API_VERSION_MINOR(instance_version),
      VK_API_VERSION_PATCH(instance_version), properties.deviceName,
      ycbcr.samplerYcbcrConversion,
      has_extension("VK_EXT_external_memory_metal"),
      has_extension("VK_ANDROID_external_memory_android_hardware_buffer"));
  vkDestroyInstance(instance, nullptr);
  return ycbcr.samplerYcbcrConversion == VK_TRUE ? 0 : 6;
}
