// Standalone Vulkan Android-WSI contract smoke test.
//
// This intentionally loads the runtime and virtual libvulkan through their
// public boundaries.  It does not construct fake AHardwareBuffers or signal
// completion in userspace: every image is cleared by a real Vulkan queue and
// every presentation is returned to the real native-window buffer queue.
#define VK_USE_PLATFORM_ANDROID_KHR 1
#include <vulkan/vulkan.h>

#include <dlfcn.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

namespace {

using PlatformSymbol = void* (*)(const char*);
using BionicDlopen = void* (*)(const char*, int);
using BionicDlsym = void* (*)(void*, const char*);
using CreateWindow = void* (*)(int32_t, int32_t, int32_t);
using ReleaseWindow = void (*)(void*);
using ResizeWindow = int32_t (*)(void*, int32_t, int32_t, int32_t);

[[noreturn]] void Fail(const char* operation, VkResult result) {
  std::fprintf(stderr, "vulkan-wsi-smoke: %s failed (%d)\n", operation,
               static_cast<int>(result));
  std::exit(1);
}
void Check(const char* operation, VkResult result) {
  if (result != VK_SUCCESS) Fail(operation, result);
}
template <class T> T Required(void* handle, const char* name) {
  auto value = reinterpret_cast<T>(dlsym(handle, name));
  if (value == nullptr) {
    std::fprintf(stderr, "vulkan-wsi-smoke: missing runtime symbol %s\n", name);
    std::exit(2);
  }
  return value;
}
template <class T> T Platform(PlatformSymbol lookup, const char* name) {
  auto value = reinterpret_cast<T>(lookup(name));
  if (value == nullptr) {
    std::fprintf(stderr, "vulkan-wsi-smoke: missing platform symbol %s\n", name);
    std::exit(2);
  }
  return value;
}

struct Api {
  PFN_vkGetInstanceProcAddr get_instance = nullptr;
  PFN_vkGetDeviceProcAddr get_device = nullptr;
  PFN_vkCreateInstance create_instance = nullptr;
  PFN_vkDestroyInstance destroy_instance = nullptr;
  PFN_vkEnumeratePhysicalDevices enumerate_physical_devices = nullptr;
  PFN_vkGetPhysicalDeviceQueueFamilyProperties get_queue_properties = nullptr;
  PFN_vkGetPhysicalDeviceSurfaceSupportKHR surface_support = nullptr;
  PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR surface_caps = nullptr;
  PFN_vkGetPhysicalDeviceSurfaceFormatsKHR surface_formats = nullptr;
  PFN_vkGetPhysicalDeviceSurfacePresentModesKHR surface_modes = nullptr;
  PFN_vkCreateAndroidSurfaceKHR create_surface = nullptr;
  PFN_vkDestroySurfaceKHR destroy_surface = nullptr;
  PFN_vkCreateDevice create_device = nullptr;
  PFN_vkDestroyDevice destroy_device = nullptr;
  PFN_vkGetDeviceQueue get_queue = nullptr;
  PFN_vkCreateSwapchainKHR create_swapchain = nullptr;
  PFN_vkDestroySwapchainKHR destroy_swapchain = nullptr;
  PFN_vkGetSwapchainImagesKHR get_images = nullptr;
  PFN_vkAcquireNextImageKHR acquire = nullptr;
  PFN_vkQueuePresentKHR present = nullptr;
  PFN_vkCreateSemaphore create_semaphore = nullptr;
  PFN_vkDestroySemaphore destroy_semaphore = nullptr;
  PFN_vkCreateFence create_fence = nullptr;
  PFN_vkDestroyFence destroy_fence = nullptr;
  PFN_vkResetFences reset_fences = nullptr;
  PFN_vkWaitForFences wait_fences = nullptr;
  PFN_vkCreateCommandPool create_command_pool = nullptr;
  PFN_vkDestroyCommandPool destroy_command_pool = nullptr;
  PFN_vkAllocateCommandBuffers allocate_command_buffers = nullptr;
  PFN_vkFreeCommandBuffers free_command_buffers = nullptr;
  PFN_vkResetCommandBuffer reset_command_buffer = nullptr;
  PFN_vkBeginCommandBuffer begin_command_buffer = nullptr;
  PFN_vkEndCommandBuffer end_command_buffer = nullptr;
  PFN_vkCmdPipelineBarrier cmd_pipeline_barrier = nullptr;
  PFN_vkCmdClearColorImage cmd_clear_color_image = nullptr;
  PFN_vkQueueSubmit queue_submit = nullptr;
  PFN_vkQueueWaitIdle queue_wait_idle = nullptr;
};

template <class T> T I(Api& api, VkInstance instance, const char* name) {
  return reinterpret_cast<T>(api.get_instance(instance, name));
}
template <class T> T D(Api& api, VkDevice device, const char* name) {
  return reinterpret_cast<T>(api.get_device(device, name));
}

void LoadInstance(Api& api, VkInstance instance) {
  api.destroy_instance = I<PFN_vkDestroyInstance>(api, instance, "vkDestroyInstance");
  api.enumerate_physical_devices = I<PFN_vkEnumeratePhysicalDevices>(api, instance, "vkEnumeratePhysicalDevices");
  api.get_queue_properties = I<PFN_vkGetPhysicalDeviceQueueFamilyProperties>(api, instance, "vkGetPhysicalDeviceQueueFamilyProperties");
  api.surface_support = I<PFN_vkGetPhysicalDeviceSurfaceSupportKHR>(api, instance, "vkGetPhysicalDeviceSurfaceSupportKHR");
  api.surface_caps = I<PFN_vkGetPhysicalDeviceSurfaceCapabilitiesKHR>(api, instance, "vkGetPhysicalDeviceSurfaceCapabilitiesKHR");
  api.surface_formats = I<PFN_vkGetPhysicalDeviceSurfaceFormatsKHR>(api, instance, "vkGetPhysicalDeviceSurfaceFormatsKHR");
  api.surface_modes = I<PFN_vkGetPhysicalDeviceSurfacePresentModesKHR>(api, instance, "vkGetPhysicalDeviceSurfacePresentModesKHR");
  api.create_surface = I<PFN_vkCreateAndroidSurfaceKHR>(api, instance, "vkCreateAndroidSurfaceKHR");
  api.destroy_surface = I<PFN_vkDestroySurfaceKHR>(api, instance, "vkDestroySurfaceKHR");
  api.create_device = I<PFN_vkCreateDevice>(api, instance, "vkCreateDevice");
}

void LoadDevice(Api& api, VkDevice device) {
  api.get_device = reinterpret_cast<PFN_vkGetDeviceProcAddr>(api.get_instance(reinterpret_cast<VkInstance>(device), "vkGetDeviceProcAddr"));
#define LOAD(field, type, name) api.field = D<type>(api, device, name)
  LOAD(destroy_device, PFN_vkDestroyDevice, "vkDestroyDevice");
  LOAD(get_queue, PFN_vkGetDeviceQueue, "vkGetDeviceQueue");
  LOAD(create_swapchain, PFN_vkCreateSwapchainKHR, "vkCreateSwapchainKHR");
  LOAD(destroy_swapchain, PFN_vkDestroySwapchainKHR, "vkDestroySwapchainKHR");
  LOAD(get_images, PFN_vkGetSwapchainImagesKHR, "vkGetSwapchainImagesKHR");
  LOAD(acquire, PFN_vkAcquireNextImageKHR, "vkAcquireNextImageKHR");
  LOAD(present, PFN_vkQueuePresentKHR, "vkQueuePresentKHR");
  LOAD(create_semaphore, PFN_vkCreateSemaphore, "vkCreateSemaphore");
  LOAD(destroy_semaphore, PFN_vkDestroySemaphore, "vkDestroySemaphore");
  LOAD(create_fence, PFN_vkCreateFence, "vkCreateFence");
  LOAD(destroy_fence, PFN_vkDestroyFence, "vkDestroyFence");
  LOAD(reset_fences, PFN_vkResetFences, "vkResetFences");
  LOAD(wait_fences, PFN_vkWaitForFences, "vkWaitForFences");
  LOAD(create_command_pool, PFN_vkCreateCommandPool, "vkCreateCommandPool");
  LOAD(destroy_command_pool, PFN_vkDestroyCommandPool, "vkDestroyCommandPool");
  LOAD(allocate_command_buffers, PFN_vkAllocateCommandBuffers, "vkAllocateCommandBuffers");
  LOAD(free_command_buffers, PFN_vkFreeCommandBuffers, "vkFreeCommandBuffers");
  LOAD(reset_command_buffer, PFN_vkResetCommandBuffer, "vkResetCommandBuffer");
  LOAD(begin_command_buffer, PFN_vkBeginCommandBuffer, "vkBeginCommandBuffer");
  LOAD(end_command_buffer, PFN_vkEndCommandBuffer, "vkEndCommandBuffer");
  LOAD(cmd_pipeline_barrier, PFN_vkCmdPipelineBarrier, "vkCmdPipelineBarrier");
  LOAD(cmd_clear_color_image, PFN_vkCmdClearColorImage, "vkCmdClearColorImage");
  LOAD(queue_submit, PFN_vkQueueSubmit, "vkQueueSubmit");
  LOAD(queue_wait_idle, PFN_vkQueueWaitIdle, "vkQueueWaitIdle");
#undef LOAD
}

void ClearImage(Api& api, VkCommandBuffer command, VkImage image,
                VkImageLayout old_layout, float shade) {
  VkImageMemoryBarrier to_transfer{
      VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER, nullptr, VK_ACCESS_MEMORY_READ_BIT,
      VK_ACCESS_TRANSFER_WRITE_BIT, old_layout, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
      VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED, image,
      {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
  api.cmd_pipeline_barrier(command, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                           VK_PIPELINE_STAGE_TRANSFER_BIT, 0, 0, nullptr, 0,
                           nullptr, 1, &to_transfer);
  VkClearColorValue color{{shade, 0.16f, 1.0f - shade, 1.0f}};
  VkImageSubresourceRange range{VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
  api.cmd_clear_color_image(command, image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                            &color, 1, &range);
  VkImageMemoryBarrier to_present{
      VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER, nullptr,
      VK_ACCESS_TRANSFER_WRITE_BIT,
      VK_ACCESS_MEMORY_READ_BIT, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
      VK_IMAGE_LAYOUT_PRESENT_SRC_KHR, VK_QUEUE_FAMILY_IGNORED,
      VK_QUEUE_FAMILY_IGNORED, image,
      {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
  api.cmd_pipeline_barrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT,
                           VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, nullptr, 0,
                           nullptr, 1, &to_present);
}

}  // namespace

int main(int argc, char** argv) {
  const char* runtime_path = argc > 1 ? argv[1] : std::getenv("AIM_RUNTIME");
  if (runtime_path == nullptr) {
    std::fprintf(stderr, "usage: %s RUNTIME_GRAPHICS_DYLIB\n", argv[0]);
    return 2;
  }
  void* runtime = dlopen(runtime_path, RTLD_NOW | RTLD_GLOBAL);
  if (runtime == nullptr) {
    std::fprintf(stderr, "vulkan-wsi-smoke: dlopen %s: %s\n", runtime_path, dlerror());
    return 2;
  }
  auto platform = Required<PlatformSymbol>(runtime, "aim_android_platform_symbol");
  auto bionic_open = Required<BionicDlopen>(runtime, "aim_bionic_dlopen");
  auto bionic_lookup = Required<BionicDlsym>(runtime, "aim_bionic_dlsym");
  void* vulkan_dso = bionic_open("libvulkan.so", 2);
  if (vulkan_dso == nullptr) return 3;
  auto gip = reinterpret_cast<PFN_vkGetInstanceProcAddr>(bionic_lookup(vulkan_dso, "vkGetInstanceProcAddr"));
  if (gip == nullptr) return 3;
  Api api;
  api.get_instance = gip;
  api.create_instance = reinterpret_cast<PFN_vkCreateInstance>(gip(nullptr, "vkCreateInstance"));
  if (api.create_instance == nullptr) return 3;

  const char* instance_extensions[] = {VK_KHR_SURFACE_EXTENSION_NAME,
                                       VK_KHR_ANDROID_SURFACE_EXTENSION_NAME};
  VkApplicationInfo app{VK_STRUCTURE_TYPE_APPLICATION_INFO, nullptr, "wsi-smoke", 1,
                        "aim", 1, VK_API_VERSION_1_0};
  VkInstanceCreateInfo instance_info{VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, nullptr,
                                     0, &app, 0, nullptr, 2, instance_extensions};
  VkInstance instance = VK_NULL_HANDLE;
  Check("vkCreateInstance", api.create_instance(&instance_info, nullptr, &instance));
  LoadInstance(api, instance);

  auto create_window = reinterpret_cast<CreateWindow>(dlsym(runtime, "aim_android_ANativeWindow_create"));
  auto release_window = Platform<ReleaseWindow>(platform, "ANativeWindow_release");
  auto resize_window = Platform<ResizeWindow>(platform, "ANativeWindow_setBuffersGeometry");
  if (create_window == nullptr) return 3;
  void* window = create_window(64, 64, 1);
  if (window == nullptr) return 3;
  VkAndroidSurfaceCreateInfoKHR surface_info{VK_STRUCTURE_TYPE_ANDROID_SURFACE_CREATE_INFO_KHR,
                                             nullptr, 0,
                                             static_cast<ANativeWindow*>(window)};
  VkSurfaceKHR surface = VK_NULL_HANDLE;
  Check("vkCreateAndroidSurfaceKHR", api.create_surface(instance, &surface_info, nullptr, &surface));

  uint32_t physical_count = 0;
  Check("vkEnumeratePhysicalDevices(count)", api.enumerate_physical_devices(instance, &physical_count, nullptr));
  std::vector<VkPhysicalDevice> physicals(physical_count);
  Check("vkEnumeratePhysicalDevices", api.enumerate_physical_devices(instance, &physical_count, physicals.data()));
  if (physicals.empty()) return 3;
  VkPhysicalDevice physical = physicals.front();
  uint32_t queue_count = 0;
  api.get_queue_properties(physical, &queue_count, nullptr);
  std::vector<VkQueueFamilyProperties> queues(queue_count);
  api.get_queue_properties(physical, &queue_count, queues.data());
  uint32_t family = UINT32_MAX;
  for (uint32_t i = 0; i < queue_count; ++i) {
    VkBool32 supported = VK_FALSE;
    Check("vkGetPhysicalDeviceSurfaceSupportKHR", api.surface_support(physical, i, surface, &supported));
    if (supported && (queues[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) { family = i; break; }
  }
  if (family == UINT32_MAX) return 3;
  VkSurfaceCapabilitiesKHR caps{};
  Check("vkGetPhysicalDeviceSurfaceCapabilitiesKHR", api.surface_caps(physical, surface, &caps));
  uint32_t format_count = 0;
  Check("vkGetPhysicalDeviceSurfaceFormatsKHR(count)", api.surface_formats(physical, surface, &format_count, nullptr));
  std::vector<VkSurfaceFormatKHR> formats(format_count);
  Check("vkGetPhysicalDeviceSurfaceFormatsKHR", api.surface_formats(physical, surface, &format_count, formats.data()));
  if (format_count != 1 || formats[0].format != VK_FORMAT_R8G8B8A8_UNORM) return 3;
  uint32_t mode_count = 0;
  Check("vkGetPhysicalDeviceSurfacePresentModesKHR(count)", api.surface_modes(physical, surface, &mode_count, nullptr));
  std::vector<VkPresentModeKHR> modes(mode_count);
  Check("vkGetPhysicalDeviceSurfacePresentModesKHR", api.surface_modes(physical, surface, &mode_count, modes.data()));
  if (mode_count != 1 || modes[0] != VK_PRESENT_MODE_FIFO_KHR || caps.minImageCount != 3 || caps.maxImageCount != 3) return 3;

  float priority = 1.0f;
  VkDeviceQueueCreateInfo queue_info{VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO, nullptr, 0, family, 1, &priority};
  const char* device_extensions[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME,
                                     VK_ANDROID_EXTERNAL_MEMORY_ANDROID_HARDWARE_BUFFER_EXTENSION_NAME,
                                     VK_EXT_QUEUE_FAMILY_FOREIGN_EXTENSION_NAME,
                                     VK_KHR_EXTERNAL_SEMAPHORE_FD_EXTENSION_NAME};
  VkDeviceCreateInfo device_info{VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO, nullptr, 0, 1, &queue_info,
                                 0, nullptr, 4, device_extensions, nullptr};
  VkDevice device = VK_NULL_HANDLE;
  Check("vkCreateDevice", api.create_device(physical, &device_info, nullptr, &device));
  LoadDevice(api, device);
  VkQueue queue = VK_NULL_HANDLE;
  api.get_queue(device, family, 0, &queue);

  VkSwapchainCreateInfoKHR swap_info{VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR, nullptr, 0, surface, 3,
                                     VK_FORMAT_R8G8B8A8_UNORM, VK_COLOR_SPACE_SRGB_NONLINEAR_KHR,
                                     {64, 64}, 1, VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT,
                                     VK_SHARING_MODE_EXCLUSIVE, 0, nullptr, VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR,
                                     VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR, VK_PRESENT_MODE_FIFO_KHR, VK_TRUE, VK_NULL_HANDLE};
  VkSwapchainKHR swapchain = VK_NULL_HANDLE;
  Check("vkCreateSwapchainKHR", api.create_swapchain(device, &swap_info, nullptr, &swapchain));
  uint32_t image_count = 0;
  Check("vkGetSwapchainImagesKHR(count)", api.get_images(device, swapchain, &image_count, nullptr));
  if (image_count != 3) return 3;
  std::vector<VkImage> images(image_count);
  Check("vkGetSwapchainImagesKHR", api.get_images(device, swapchain, &image_count, images.data()));

  VkCommandPoolCreateInfo pool_info{VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, nullptr,
                                    VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT, family};
  VkCommandPool pool = VK_NULL_HANDLE;
  Check("vkCreateCommandPool", api.create_command_pool(device, &pool_info, nullptr, &pool));
  std::vector<VkCommandBuffer> commands(3);
  VkCommandBufferAllocateInfo alloc_info{VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO, nullptr,
                                         pool, VK_COMMAND_BUFFER_LEVEL_PRIMARY, 3};
  Check("vkAllocateCommandBuffers", api.allocate_command_buffers(device, &alloc_info, commands.data()));
  VkSemaphoreCreateInfo sem_info{VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO, nullptr, 0};
  VkFenceCreateInfo fence_info{VK_STRUCTURE_TYPE_FENCE_CREATE_INFO, nullptr, 0};
  VkSemaphore acquire_sem = VK_NULL_HANDLE, render_sem = VK_NULL_HANDLE;
  VkFence acquire_fence = VK_NULL_HANDLE, submit_fence = VK_NULL_HANDLE;
  Check("vkCreateSemaphore(acquire)", api.create_semaphore(device, &sem_info, nullptr, &acquire_sem));
  Check("vkCreateSemaphore(render)", api.create_semaphore(device, &sem_info, nullptr, &render_sem));
  Check("vkCreateFence(acquire)", api.create_fence(device, &fence_info, nullptr, &acquire_fence));
  Check("vkCreateFence(submit)", api.create_fence(device, &fence_info, nullptr, &submit_fence));
  std::vector<VkImageLayout> layouts(3, VK_IMAGE_LAYOUT_UNDEFINED);
  for (uint32_t frame = 0; frame < 12; ++frame) {
    Check("vkResetFences(acquire)", api.reset_fences(device, 1, &acquire_fence));
    uint32_t index = 0;
    Check("vkAcquireNextImageKHR", api.acquire(device, swapchain, UINT64_MAX, acquire_sem, acquire_fence, &index));
    Check("vkWaitForFences(acquire)", api.wait_fences(device, 1, &acquire_fence, VK_TRUE, 0));
    Check("vkResetCommandBuffer", api.reset_command_buffer(commands[index], 0));
    VkCommandBufferBeginInfo begin{VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO, nullptr, VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT, nullptr};
    Check("vkBeginCommandBuffer", api.begin_command_buffer(commands[index], &begin));
    ClearImage(api, commands[index], images[index], layouts[index], (frame % 4) * 0.2f);
    layouts[index] = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    Check("vkEndCommandBuffer", api.end_command_buffer(commands[index]));
    Check("vkResetFences(submit)", api.reset_fences(device, 1, &submit_fence));
    VkPipelineStageFlags stage = VK_PIPELINE_STAGE_TRANSFER_BIT;
    VkSubmitInfo submit{VK_STRUCTURE_TYPE_SUBMIT_INFO, nullptr, 1, &acquire_sem, &stage, 1, &commands[index], 1, &render_sem};
    Check("vkQueueSubmit", api.queue_submit(queue, 1, &submit, submit_fence));
    VkPresentInfoKHR present{VK_STRUCTURE_TYPE_PRESENT_INFO_KHR, nullptr, 1, &render_sem, 1, &swapchain, &index, nullptr};
    Check("vkQueuePresentKHR", api.present(queue, &present));
    Check("vkQueueWaitIdle", api.queue_wait_idle(queue));
  }

  // Same-size recreation exercises image retirement while preserving the
  // native window's three identities.
  VkSwapchainKHR replacement = VK_NULL_HANDLE;
  swap_info.oldSwapchain = swapchain;
  Check("vkCreateSwapchainKHR(recreate)", api.create_swapchain(device, &swap_info, nullptr, &replacement));
  api.destroy_swapchain(device, swapchain, nullptr);
  swapchain = replacement;
  Check("ANativeWindow_setBuffersGeometry(resize)", static_cast<VkResult>(resize_window(window, 72, 72, 1)));
  swap_info.surface = surface;
  swap_info.imageExtent = {72, 72};
  swap_info.oldSwapchain = swapchain;
  VkSwapchainKHR resized = VK_NULL_HANDLE;
  VkResult resize_result = api.create_swapchain(device, &swap_info, nullptr, &resized);
  if (resize_result != VK_SUCCESS && resize_result != VK_ERROR_OUT_OF_DATE_KHR) Fail("vkCreateSwapchainKHR(resize)", resize_result);
  if (resized != VK_NULL_HANDLE) api.destroy_swapchain(device, resized, nullptr);
  api.destroy_swapchain(device, swapchain, nullptr);
  api.free_command_buffers(device, pool, 3, commands.data());
  api.destroy_command_pool(device, pool, nullptr);
  api.destroy_fence(device, submit_fence, nullptr);
  api.destroy_fence(device, acquire_fence, nullptr);
  api.destroy_semaphore(device, render_sem, nullptr);
  api.destroy_semaphore(device, acquire_sem, nullptr);
  api.destroy_device(device, nullptr);
  api.destroy_surface(instance, surface, nullptr);
  release_window(window);
  api.destroy_instance(instance, nullptr);
  std::puts("Vulkan Android WSI smoke PASS: 3 images, 12 GPU clear/present frames, same-size recreation, resize=out-of-date-safe");
}
