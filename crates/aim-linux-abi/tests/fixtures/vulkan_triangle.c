// An NDK program for tests/vulkan.rs: Vulkan through the original
// libvulkan loader and the vendor driver (vulkan.aim.so, MoltenVK on the
// host), with AHardwareBuffers from the allocator and mapper HALs.
//
//   vulkan_triangle        instance and device, a triangle into an
//                          AHardwareBuffer-backed image read back on the CPU,
//                          and sync-fd semaphores
//   vulkan_triangle swapchain
//                          a swapchain on an ImageReader's window; the
//                          loader asks SurfaceFlinger for the refresh
//                          period, so this needs a booted guest
//   vulkan_triangle info   what vulkaninfo reports: the device, its API
//                          version, extensions and missing core features
//
// Prints one "ok ..." line per check and "timing ..." lines; exits non-zero
// on the first failure.

#include <android/hardware_buffer.h>
#include <android/native_window.h>
#include <media/NdkImageReader.h>
#define VK_USE_PLATFORM_ANDROID_KHR
#include <vulkan/vulkan.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define W 64
#define H 64

#define CHECK(cond, ...)                   \
    do {                                   \
        if (!(cond)) {                     \
            printf("FAIL: " __VA_ARGS__);  \
            printf("\n");                  \
            exit(1);                       \
        }                                  \
    } while (0)
#define VK(call) CHECK((r = (call)) == VK_SUCCESS, #call " = %d", r)

static double now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1e9 + ts.tv_nsec;
}

// glslc -O --target-env=vulkan1.1 of:
//   #version 450
//   void main() {
//       vec2 p[3] = vec2[](vec2(0.0, -0.5), vec2(0.5, 0.5), vec2(-0.5, 0.5));
//       gl_Position = vec4(p[gl_VertexIndex], 0.0, 1.0);
//   }
static const uint32_t kVert[] = {
    0x07230203, 0x00010300, 0x000d000a, 0x00000028, 0x00000000, 0x00020011,
    0x00000001, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e,
    0x00000000, 0x0003000e, 0x00000000, 0x00000001, 0x0007000f, 0x00000000,
    0x00000004, 0x6e69616d, 0x00000000, 0x00000019, 0x0000001d, 0x00050048,
    0x00000017, 0x00000000, 0x0000000b, 0x00000000, 0x00050048, 0x00000017,
    0x00000001, 0x0000000b, 0x00000001, 0x00050048, 0x00000017, 0x00000002,
    0x0000000b, 0x00000003, 0x00050048, 0x00000017, 0x00000003, 0x0000000b,
    0x00000004, 0x00030047, 0x00000017, 0x00000002, 0x00040047, 0x0000001d,
    0x0000000b, 0x0000002a, 0x00020013, 0x00000002, 0x00030021, 0x00000003,
    0x00000002, 0x00030016, 0x00000006, 0x00000020, 0x00040017, 0x00000007,
    0x00000006, 0x00000002, 0x00040015, 0x00000008, 0x00000020, 0x00000000,
    0x0004002b, 0x00000008, 0x00000009, 0x00000003, 0x0004001c, 0x0000000a,
    0x00000007, 0x00000009, 0x00040020, 0x0000000b, 0x00000007, 0x0000000a,
    0x0004002b, 0x00000006, 0x0000000d, 0x00000000, 0x0004002b, 0x00000006,
    0x0000000e, 0xbf000000, 0x0005002c, 0x00000007, 0x0000000f, 0x0000000d,
    0x0000000e, 0x0004002b, 0x00000006, 0x00000010, 0x3f000000, 0x0005002c,
    0x00000007, 0x00000011, 0x00000010, 0x00000010, 0x0005002c, 0x00000007,
    0x00000012, 0x0000000e, 0x00000010, 0x0006002c, 0x0000000a, 0x00000013,
    0x0000000f, 0x00000011, 0x00000012, 0x00040017, 0x00000014, 0x00000006,
    0x00000004, 0x0004002b, 0x00000008, 0x00000015, 0x00000001, 0x0004001c,
    0x00000016, 0x00000006, 0x00000015, 0x0006001e, 0x00000017, 0x00000014,
    0x00000006, 0x00000016, 0x00000016, 0x00040020, 0x00000018, 0x00000003,
    0x00000017, 0x0004003b, 0x00000018, 0x00000019, 0x00000003, 0x00040015,
    0x0000001a, 0x00000020, 0x00000001, 0x0004002b, 0x0000001a, 0x0000001b,
    0x00000000, 0x00040020, 0x0000001c, 0x00000001, 0x0000001a, 0x0004003b,
    0x0000001c, 0x0000001d, 0x00000001, 0x00040020, 0x0000001f, 0x00000007,
    0x00000007, 0x0004002b, 0x00000006, 0x00000022, 0x3f800000, 0x00040020,
    0x00000026, 0x00000003, 0x00000014, 0x00050036, 0x00000002, 0x00000004,
    0x00000000, 0x00000003, 0x000200f8, 0x00000005, 0x0004003b, 0x0000000b,
    0x0000000c, 0x00000007, 0x0003003e, 0x0000000c, 0x00000013, 0x0004003d,
    0x0000001a, 0x0000001e, 0x0000001d, 0x00050041, 0x0000001f, 0x00000020,
    0x0000000c, 0x0000001e, 0x0004003d, 0x00000007, 0x00000021, 0x00000020,
    0x00050051, 0x00000006, 0x00000023, 0x00000021, 0x00000000, 0x00050051,
    0x00000006, 0x00000024, 0x00000021, 0x00000001, 0x00070050, 0x00000014,
    0x00000025, 0x00000023, 0x00000024, 0x0000000d, 0x00000022, 0x00050041,
    0x00000026, 0x00000027, 0x00000019, 0x0000001b, 0x0003003e, 0x00000027,
    0x00000025, 0x000100fd, 0x00010038,
};
// glslc -O --target-env=vulkan1.1 of:
//   #version 450
//   layout(location = 0) out vec4 color;
//   void main() { color = vec4(1.0, 0.0, 0.0, 1.0); }
static const uint32_t kFrag[] = {
    0x07230203, 0x00010300, 0x000d000a, 0x0000000d, 0x00000000, 0x00020011,
    0x00000001, 0x0006000b, 0x00000001, 0x4c534c47, 0x6474732e, 0x3035342e,
    0x00000000, 0x0003000e, 0x00000000, 0x00000001, 0x0006000f, 0x00000004,
    0x00000004, 0x6e69616d, 0x00000000, 0x00000009, 0x00030010, 0x00000004,
    0x00000007, 0x00040047, 0x00000009, 0x0000001e, 0x00000000, 0x00020013,
    0x00000002, 0x00030021, 0x00000003, 0x00000002, 0x00030016, 0x00000006,
    0x00000020, 0x00040017, 0x00000007, 0x00000006, 0x00000004, 0x00040020,
    0x00000008, 0x00000003, 0x00000007, 0x0004003b, 0x00000008, 0x00000009,
    0x00000003, 0x0004002b, 0x00000006, 0x0000000a, 0x3f800000, 0x0004002b,
    0x00000006, 0x0000000b, 0x00000000, 0x0007002c, 0x00000007, 0x0000000c,
    0x0000000a, 0x0000000b, 0x0000000b, 0x0000000a, 0x00050036, 0x00000002,
    0x00000004, 0x00000000, 0x00000003, 0x000200f8, 0x00000005, 0x0003003e,
    0x00000009, 0x0000000c, 0x000100fd, 0x00010038,
};

static VkResult r;
static VkInstance instance;
static VkPhysicalDevice gpu;
static VkDevice device;
static VkQueue queue;
// The second queue of the graphics family, as HWUI asks for two.
static VkQueue queue2;
static VkCommandPool pool;
static VkRenderPass pass;
static VkPipelineLayout layout;
static VkPipeline pipeline;

static int has(const VkExtensionProperties* e, uint32_t n, const char* name) {
    for (uint32_t i = 0; i < n; i++)
        if (!strcmp(e[i].extensionName, name)) return 1;
    return 0;
}

static void create_instance(void) {
    VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
                             .pApplicationName = "vulkan_triangle",
                             .apiVersion = VK_API_VERSION_1_3};
    const char* exts[] = {VK_KHR_SURFACE_EXTENSION_NAME, VK_KHR_ANDROID_SURFACE_EXTENSION_NAME};
    VkInstanceCreateInfo info = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
                                 .pApplicationInfo = &app,
                                 .enabledExtensionCount = 2,
                                 .ppEnabledExtensionNames = exts};
    VK(vkCreateInstance(&info, NULL, &instance));
    uint32_t n = 1;
    r = vkEnumeratePhysicalDevices(instance, &n, &gpu);
    CHECK((r == VK_SUCCESS || r == VK_INCOMPLETE) && n == 1, "no physical device (%d)", r);
}

// Every VkBool32 of VkPhysicalDeviceFeatures that is false.
static void missing_features(void) {
    static const char* names[] = {
        "robustBufferAccess", "fullDrawIndexUint32", "imageCubeArray", "independentBlend",
        "geometryShader", "tessellationShader", "sampleRateShading", "dualSrcBlend", "logicOp",
        "multiDrawIndirect", "drawIndirectFirstInstance", "depthClamp", "depthBiasClamp",
        "fillModeNonSolid", "depthBounds", "wideLines", "largePoints", "alphaToOne",
        "multiViewport", "samplerAnisotropy", "textureCompressionETC2",
        "textureCompressionASTC_LDR", "textureCompressionBC", "occlusionQueryPrecise",
        "pipelineStatisticsQuery", "vertexPipelineStoresAndAtomics", "fragmentStoresAndAtomics",
        "shaderTessellationAndGeometryPointSize", "shaderImageGatherExtended",
        "shaderStorageImageExtendedFormats", "shaderStorageImageMultisample",
        "shaderStorageImageReadWithoutFormat", "shaderStorageImageWriteWithoutFormat",
        "shaderUniformBufferArrayDynamicIndexing", "shaderSampledImageArrayDynamicIndexing",
        "shaderStorageBufferArrayDynamicIndexing", "shaderStorageImageArrayDynamicIndexing",
        "shaderClipDistance", "shaderCullDistance", "shaderFloat64", "shaderInt64",
        "shaderInt16", "shaderResourceResidency", "shaderResourceMinLod", "sparseBinding",
        "sparseResidencyBuffer", "sparseResidencyImage2D", "sparseResidencyImage3D",
        "sparseResidency2Samples", "sparseResidency4Samples", "sparseResidency8Samples",
        "sparseResidency16Samples", "sparseResidencyAliased", "variableMultisampleRate",
        "inheritedQueries"};
    VkPhysicalDeviceFeatures f;
    vkGetPhysicalDeviceFeatures(gpu, &f);
    const VkBool32* b = (const VkBool32*)&f;
    printf("missing features:");
    for (unsigned i = 0; i < sizeof(names) / sizeof(names[0]); i++)
        if (!b[i]) printf(" %s", names[i]);
    printf("\n");
}

static int info(void) {
    create_instance();
    VkPhysicalDeviceProperties p;
    vkGetPhysicalDeviceProperties(gpu, &p);
    uint32_t api = 0;
    vkEnumerateInstanceVersion(&api);
    printf("instance version %u.%u.%u\n", VK_VERSION_MAJOR(api), VK_VERSION_MINOR(api),
           VK_VERSION_PATCH(api));
    printf("device %s: Vulkan %u.%u.%u, driver %#x, vendor %#x, device %#x, type %d\n",
           p.deviceName, VK_VERSION_MAJOR(p.apiVersion), VK_VERSION_MINOR(p.apiVersion),
           VK_VERSION_PATCH(p.apiVersion), p.driverVersion, p.vendorID, p.deviceID,
           p.deviceType);
    uint32_t n = 0;
    vkEnumerateInstanceExtensionProperties(NULL, &n, NULL);
    VkExtensionProperties* e = calloc(n, sizeof(*e));
    vkEnumerateInstanceExtensionProperties(NULL, &n, e);
    printf("instance extensions (%u):", n);
    for (uint32_t i = 0; i < n; i++) printf(" %s", e[i].extensionName);
    printf("\n");
    free(e);
    vkEnumerateDeviceExtensionProperties(gpu, NULL, &n, NULL);
    e = calloc(n, sizeof(*e));
    vkEnumerateDeviceExtensionProperties(gpu, NULL, &n, e);
    printf("device extensions (%u):", n);
    for (uint32_t i = 0; i < n; i++) printf(" %s", e[i].extensionName);
    printf("\n");
    free(e);
    missing_features();
    printf("limits: maxImageDimension2D %u, maxBoundDescriptorSets %u, "
           "maxPerStageDescriptorSamplers %u, maxComputeWorkGroupInvocations %u\n",
           p.limits.maxImageDimension2D, p.limits.maxBoundDescriptorSets,
           p.limits.maxPerStageDescriptorSamplers, p.limits.maxComputeWorkGroupInvocations);
    vkDestroyInstance(instance, NULL);
    printf("ok info\n");
    return 0;
}

static VkShaderModule shader(const uint32_t* code, size_t size) {
    VkShaderModuleCreateInfo info = {.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
                                     .codeSize = size,
                                     .pCode = code};
    VkShaderModule m;
    VK(vkCreateShaderModule(device, &info, NULL, &m));
    return m;
}

// A render pass that clears one R8G8B8A8 attachment and leaves it in
// `final_layout`, and a pipeline drawing the red triangle.
static void create_pipeline(VkImageLayout final_layout) {
    VkAttachmentDescription a = {.format = VK_FORMAT_R8G8B8A8_UNORM,
                                 .samples = VK_SAMPLE_COUNT_1_BIT,
                                 .loadOp = VK_ATTACHMENT_LOAD_OP_CLEAR,
                                 .storeOp = VK_ATTACHMENT_STORE_OP_STORE,
                                 .stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE,
                                 .stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE,
                                 .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
                                 .finalLayout = final_layout};
    VkAttachmentReference ref = {0, VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};
    VkSubpassDescription sub = {.pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS,
                                .colorAttachmentCount = 1,
                                .pColorAttachments = &ref};
    VkRenderPassCreateInfo rp = {.sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
                                 .attachmentCount = 1,
                                 .pAttachments = &a,
                                 .subpassCount = 1,
                                 .pSubpasses = &sub};
    VK(vkCreateRenderPass(device, &rp, NULL, &pass));
    VkShaderModule vs = shader(kVert, sizeof(kVert)), fs = shader(kFrag, sizeof(kFrag));
    VkPipelineShaderStageCreateInfo stages[2] = {
        {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
         .stage = VK_SHADER_STAGE_VERTEX_BIT,
         .module = vs,
         .pName = "main"},
        {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
         .stage = VK_SHADER_STAGE_FRAGMENT_BIT,
         .module = fs,
         .pName = "main"}};
    VkPipelineVertexInputStateCreateInfo vi = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
    VkPipelineInputAssemblyStateCreateInfo ia = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
        .topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST};
    VkViewport vp = {0, 0, W, H, 0, 1};
    VkRect2D sc = {{0, 0}, {W, H}};
    VkPipelineViewportStateCreateInfo vps = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
        .viewportCount = 1,
        .pViewports = &vp,
        .scissorCount = 1,
        .pScissors = &sc};
    VkPipelineRasterizationStateCreateInfo rs = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
        .polygonMode = VK_POLYGON_MODE_FILL,
        .cullMode = VK_CULL_MODE_NONE,
        .lineWidth = 1};
    VkPipelineMultisampleStateCreateInfo ms = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
        .rasterizationSamples = VK_SAMPLE_COUNT_1_BIT};
    VkPipelineColorBlendAttachmentState cba = {.colorWriteMask = 0xf};
    VkPipelineColorBlendStateCreateInfo cb = {
        .sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
        .attachmentCount = 1,
        .pAttachments = &cba};
    VkPipelineLayoutCreateInfo pl = {.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO};
    VK(vkCreatePipelineLayout(device, &pl, NULL, &layout));
    VkGraphicsPipelineCreateInfo gp = {.sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
                                       .stageCount = 2,
                                       .pStages = stages,
                                       .pVertexInputState = &vi,
                                       .pInputAssemblyState = &ia,
                                       .pViewportState = &vps,
                                       .pRasterizationState = &rs,
                                       .pMultisampleState = &ms,
                                       .pColorBlendState = &cb,
                                       .layout = layout,
                                       .renderPass = pass};
    VK(vkCreateGraphicsPipelines(device, VK_NULL_HANDLE, 1, &gp, NULL, &pipeline));
    vkDestroyShaderModule(device, vs, NULL);
    vkDestroyShaderModule(device, fs, NULL);
}

static VkImageView view_of(VkImage image) {
    VkImageViewCreateInfo info = {.sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
                                  .image = image,
                                  .viewType = VK_IMAGE_VIEW_TYPE_2D,
                                  .format = VK_FORMAT_R8G8B8A8_UNORM,
                                  .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}};
    VkImageView v;
    VK(vkCreateImageView(device, &info, NULL, &v));
    return v;
}

// Record: clear `fb` to (r, g, b) and draw the triangle.
static VkCommandBuffer record(VkFramebuffer fb, float red, float green, float blue) {
    VkCommandBufferAllocateInfo ai = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
                                      .commandPool = pool,
                                      .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
                                      .commandBufferCount = 1};
    VkCommandBuffer cmd;
    VK(vkAllocateCommandBuffers(device, &ai, &cmd));
    VkCommandBufferBeginInfo bi = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
    VK(vkBeginCommandBuffer(cmd, &bi));
    VkClearValue clear = {.color = {{red, green, blue, 1.0f}}};
    VkRenderPassBeginInfo rb = {.sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
                                .renderPass = pass,
                                .framebuffer = fb,
                                .renderArea = {{0, 0}, {W, H}},
                                .clearValueCount = 1,
                                .pClearValues = &clear};
    vkCmdBeginRenderPass(cmd, &rb, VK_SUBPASS_CONTENTS_INLINE);
    vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_GRAPHICS, pipeline);
    vkCmdDraw(cmd, 3, 1, 0, 0);
    vkCmdEndRenderPass(cmd);
    VK(vkEndCommandBuffer(cmd));
    return cmd;
}

static VkFramebuffer framebuffer(VkImageView v) {
    VkFramebufferCreateInfo info = {.sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
                                    .renderPass = pass,
                                    .attachmentCount = 1,
                                    .pAttachments = &v,
                                    .width = W,
                                    .height = H,
                                    .layers = 1};
    VkFramebuffer fb;
    VK(vkCreateFramebuffer(device, &info, NULL, &fb));
    return fb;
}

static uint32_t at(const uint8_t* p, uint32_t stride_bytes, int x, int y) {
    const uint8_t* q = p + y * stride_bytes + x * 4;
    return (uint32_t)q[0] << 24 | q[1] << 16 | q[2] << 8 | q[3];
}

// Red triangle (apex at the top) on a `clear` background (RGBA, big endian).
static void check_frame(AHardwareBuffer* ahb, uint32_t clear, const char* what) {
    AHardwareBuffer_Desc d;
    AHardwareBuffer_describe(ahb, &d);
    uint8_t* cpu = NULL;
    CHECK(AHardwareBuffer_lock(ahb, AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN, -1, NULL,
                               (void**)&cpu) == 0, "%s: lock", what);
    uint32_t corner = at(cpu, d.stride * 4, 1, 1);
    uint32_t centre = at(cpu, d.stride * 4, W / 2, H / 2);
    uint32_t near_top = at(cpu, d.stride * 4, W / 2, H / 4 + 2);
    uint32_t top_left = at(cpu, d.stride * 4, W / 4, H / 4 + 2);
    AHardwareBuffer_unlock(ahb, NULL);
    CHECK(corner == clear, "%s: corner %08x, want %08x", what, corner, clear);
    CHECK(centre == 0xff0000ff, "%s: centre %08x", what, centre);
    // The apex is at the top: row H/4 is narrow, so only its middle is red.
    CHECK(near_top == 0xff0000ff && top_left == clear, "%s: orientation %08x %08x", what,
          near_top, top_left);
}

static void submit_and_wait(VkCommandBuffer cmd) {
    VkFenceCreateInfo fi = {.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    VkFence fence;
    VK(vkCreateFence(device, &fi, NULL, &fence));
    VkSubmitInfo si = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                       .commandBufferCount = 1,
                       .pCommandBuffers = &cmd};
    VK(vkQueueSubmit(queue, 1, &si, fence));
    VK(vkWaitForFences(device, 1, &fence, VK_TRUE, UINT64_MAX));
    vkDestroyFence(device, fence, NULL);
}

// A triangle into an image whose memory is an AHardwareBuffer, read back
// through the buffer's CPU mapping.
static void ahb_triangle(void) {
    AHardwareBuffer_Desc desc = {.width = W,
                                 .height = H,
                                 .layers = 1,
                                 .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
                                 .usage = AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT |
                                          AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE |
                                          AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN};
    AHardwareBuffer* ahb = NULL;
    CHECK(AHardwareBuffer_allocate(&desc, &ahb) == 0, "AHardwareBuffer_allocate");
    VkAndroidHardwareBufferFormatPropertiesANDROID fmt = {
        .sType = VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_FORMAT_PROPERTIES_ANDROID};
    VkAndroidHardwareBufferPropertiesANDROID props = {
        .sType = VK_STRUCTURE_TYPE_ANDROID_HARDWARE_BUFFER_PROPERTIES_ANDROID, .pNext = &fmt};
    VK(vkGetAndroidHardwareBufferPropertiesANDROID(device, ahb, &props));
    CHECK(fmt.format == VK_FORMAT_R8G8B8A8_UNORM && props.memoryTypeBits,
          "AHardwareBuffer properties: format %d, types %x", fmt.format, props.memoryTypeBits);
    VkExternalMemoryImageCreateInfo ext = {
        .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
        .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_ANDROID_HARDWARE_BUFFER_BIT_ANDROID};
    VkImageCreateInfo ii = {.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
                            .pNext = &ext,
                            .imageType = VK_IMAGE_TYPE_2D,
                            .format = VK_FORMAT_R8G8B8A8_UNORM,
                            .extent = {W, H, 1},
                            .mipLevels = 1,
                            .arrayLayers = 1,
                            .samples = VK_SAMPLE_COUNT_1_BIT,
                            .tiling = VK_IMAGE_TILING_OPTIMAL,
                            .usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT |
                                     VK_IMAGE_USAGE_SAMPLED_BIT,
                            .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED};
    VkImage image;
    VK(vkCreateImage(device, &ii, NULL, &image));
    VkMemoryDedicatedAllocateInfo dedicated = {
        .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO, .image = image};
    VkImportAndroidHardwareBufferInfoANDROID import = {
        .sType = VK_STRUCTURE_TYPE_IMPORT_ANDROID_HARDWARE_BUFFER_INFO_ANDROID,
        .pNext = &dedicated,
        .buffer = ahb};
    VkMemoryAllocateInfo mi = {.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
                               .pNext = &import,
                               .allocationSize = props.allocationSize,
                               .memoryTypeIndex = __builtin_ctz(props.memoryTypeBits)};
    VkDeviceMemory memory;
    VK(vkAllocateMemory(device, &mi, NULL, &memory));
    VK(vkBindImageMemory(device, image, memory, 0));
    VkImageView v = view_of(image);
    VkFramebuffer fb = framebuffer(v);
    VkCommandBuffer cmd = record(fb, 0.0f, 0.0f, 1.0f);
    double t0 = now_ns();
    submit_and_wait(cmd);
    printf("timing ahb frame (submit + wait) %.0f us\n", (now_ns() - t0) / 1e3);
    check_frame(ahb, 0x0000ffff, "AHardwareBuffer image");
    // The same memory exported again is the same buffer.
    VkMemoryGetAndroidHardwareBufferInfoANDROID gi = {
        .sType = VK_STRUCTURE_TYPE_MEMORY_GET_ANDROID_HARDWARE_BUFFER_INFO_ANDROID,
        .memory = memory};
    AHardwareBuffer* again = NULL;
    VK(vkGetMemoryAndroidHardwareBufferANDROID(device, &gi, &again));
    CHECK(again == ahb, "exported buffer %p, imported %p", (void*)again, (void*)ahb);
    AHardwareBuffer_release(again);
    // Again on the family's second queue, after a semaphore the first
    // signals, with a command buffer from the family's pool.
    VkCommandBuffer cmd2 = record(fb, 0.0f, 1.0f, 0.0f);
    VkSemaphoreCreateInfo sci = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    VkSemaphore first;
    VK(vkCreateSemaphore(device, &sci, NULL, &first));
    VkSubmitInfo signal = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                           .signalSemaphoreCount = 1,
                           .pSignalSemaphores = &first};
    VK(vkQueueSubmit(queue, 1, &signal, VK_NULL_HANDLE));
    VkFenceCreateInfo fci = {.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    VkFence done;
    VK(vkCreateFence(device, &fci, NULL, &done));
    VkPipelineStageFlags stage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
    VkSubmitInfo second = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                           .waitSemaphoreCount = 1,
                           .pWaitSemaphores = &first,
                           .pWaitDstStageMask = &stage,
                           .commandBufferCount = 1,
                           .pCommandBuffers = &cmd2};
    VK(vkQueueSubmit(queue2, 1, &second, done));
    VK(vkWaitForFences(device, 1, &done, VK_TRUE, UINT64_MAX));
    check_frame(ahb, 0x00ff00ff, "second queue");
    vkDestroyFence(device, done, NULL);
    vkDestroySemaphore(device, first, NULL);
    vkFreeCommandBuffers(device, pool, 1, &cmd2);
    printf("ok the graphics family's second queue, after a semaphore from the first\n");
    vkFreeCommandBuffers(device, pool, 1, &cmd);
    vkDestroyFramebuffer(device, fb, NULL);
    vkDestroyImageView(device, v, NULL);
    vkDestroyImage(device, image, NULL);
    vkFreeMemory(device, memory, NULL);
    AHardwareBuffer_release(ahb);
    printf("ok AHardwareBuffer image clear + triangle, read back on the CPU\n");
}

// Sync-fd semaphores, as HWUI hands frames to SurfaceFlinger and back: the
// payload of a semaphore whose signal is pending (on the second queue,
// behind a timeline semaphore the CPU signals) exported as a sync_file,
// which signals only after it, and imported into another semaphore that a
// submission waits for.
static void sync_fd(void) {
    VkExportSemaphoreCreateInfo export_info = {
        .sType = VK_STRUCTURE_TYPE_EXPORT_SEMAPHORE_CREATE_INFO,
        .handleTypes = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT};
    VkSemaphoreCreateInfo si = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO,
                                .pNext = &export_info};
    PFN_vkGetSemaphoreFdKHR get_fd =
        (PFN_vkGetSemaphoreFdKHR)vkGetDeviceProcAddr(device, "vkGetSemaphoreFdKHR");
    PFN_vkImportSemaphoreFdKHR import_fd =
        (PFN_vkImportSemaphoreFdKHR)vkGetDeviceProcAddr(device, "vkImportSemaphoreFdKHR");
    PFN_vkSignalSemaphore signal_semaphore =
        (PFN_vkSignalSemaphore)vkGetDeviceProcAddr(device, "vkSignalSemaphore");
    CHECK(get_fd && import_fd && signal_semaphore, "no sync-fd or timeline entry points");
    VkSemaphore signaled, imported, gate;
    VK(vkCreateSemaphore(device, &si, NULL, &signaled));
    VK(vkCreateSemaphore(device, &si, NULL, &imported));
    VkSemaphoreTypeCreateInfo timeline = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_TYPE_CREATE_INFO,
                                          .semaphoreType = VK_SEMAPHORE_TYPE_TIMELINE};
    VkSemaphoreCreateInfo ti = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO,
                                .pNext = &timeline};
    VK(vkCreateSemaphore(device, &ti, NULL, &gate));
    uint64_t one = 1;
    VkTimelineSemaphoreSubmitInfo values = {
        .sType = VK_STRUCTURE_TYPE_TIMELINE_SEMAPHORE_SUBMIT_INFO,
        .waitSemaphoreValueCount = 1,
        .pWaitSemaphoreValues = &one};
    VkPipelineStageFlags stage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
    VkSubmitInfo gated = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                          .pNext = &values,
                          .waitSemaphoreCount = 1,
                          .pWaitSemaphores = &gate,
                          .pWaitDstStageMask = &stage,
                          .signalSemaphoreCount = 1,
                          .pSignalSemaphores = &signaled};
    VK(vkQueueSubmit(queue2, 1, &gated, VK_NULL_HANDLE));
    VkSemaphoreGetFdInfoKHR gi = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_GET_FD_INFO_KHR,
                                  .semaphore = signaled,
                                  .handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT};
    int fd = -2;
    VK(get_fd(device, &gi, &fd));
    CHECK(fd >= 0, "sync fd %d", fd);
    struct pollfd p = {.fd = fd, .events = POLLIN};
    CHECK(poll(&p, 1, 50) == 0, "the sync_file signaled before its semaphore");
    int watch = dup(fd);
    VkImportSemaphoreFdInfoKHR ii = {.sType = VK_STRUCTURE_TYPE_IMPORT_SEMAPHORE_FD_INFO_KHR,
                                     .semaphore = imported,
                                     .flags = VK_SEMAPHORE_IMPORT_TEMPORARY_BIT,
                                     .handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
                                     .fd = fd};
    VK(import_fd(device, &ii));
    VkFenceCreateInfo fci = {.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO};
    VkFence done;
    VK(vkCreateFence(device, &fci, NULL, &done));
    VkSubmitInfo wait = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                         .waitSemaphoreCount = 1,
                         .pWaitSemaphores = &imported,
                         .pWaitDstStageMask = &stage};
    VK(vkQueueSubmit(queue, 1, &wait, done));
    CHECK((r = vkWaitForFences(device, 1, &done, VK_TRUE, 50000000)) == VK_TIMEOUT,
          "the imported semaphore signaled before the sync_file: %d", r);
    double t0 = now_ns();
    VkSemaphoreSignalInfo open = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_SIGNAL_INFO,
                                  .semaphore = gate,
                                  .value = 1};
    VK(signal_semaphore(device, &open));
    VK(vkWaitForFences(device, 1, &done, VK_TRUE, 5000000000ull));
    printf("timing sync_file round trip %.0f us\n", (now_ns() - t0) / 1e3);
    p.fd = watch;
    CHECK(poll(&p, 1, 0) == 1, "the exported sync_file never signaled");
    close(watch);
    vkDestroyFence(device, done, NULL);
    vkDestroySemaphore(device, gate, NULL);
    vkDestroySemaphore(device, signaled, NULL);
    vkDestroySemaphore(device, imported, NULL);
    printf("ok sync-fd semaphore export and import of a pending payload\n");
}

// A swapchain on an ImageReader's window (the original loader's
// VK_KHR_swapchain over our VK_ANDROID_native_buffer).
static void swapchain(void) {
    AImageReader* reader = NULL;
    CHECK(AImageReader_newWithUsage(W, H, AIMAGE_FORMAT_RGBA_8888,
                                    AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT |
                                            AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN,
                                    4, &reader) == AMEDIA_OK, "AImageReader_newWithUsage");
    ANativeWindow* window = NULL;
    CHECK(AImageReader_getWindow(reader, &window) == AMEDIA_OK, "AImageReader_getWindow");
    VkAndroidSurfaceCreateInfoKHR si = {.sType = VK_STRUCTURE_TYPE_ANDROID_SURFACE_CREATE_INFO_KHR,
                                        .window = window};
    VkSurfaceKHR surface;
    VK(vkCreateAndroidSurfaceKHR(instance, &si, NULL, &surface));
    VkBool32 supported = VK_FALSE;
    VK(vkGetPhysicalDeviceSurfaceSupportKHR(gpu, 0, surface, &supported));
    CHECK(supported, "queue family 0 cannot present");
    VkSurfaceCapabilitiesKHR caps;
    VK(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(gpu, surface, &caps));
    VkSwapchainCreateInfoKHR sc = {.sType = VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR,
                                   .surface = surface,
                                   .minImageCount = caps.minImageCount,
                                   .imageFormat = VK_FORMAT_R8G8B8A8_UNORM,
                                   .imageColorSpace = VK_COLOR_SPACE_SRGB_NONLINEAR_KHR,
                                   .imageExtent = {W, H},
                                   .imageArrayLayers = 1,
                                   .imageUsage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT,
                                   .preTransform = VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR,
                                   .compositeAlpha = VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR,
                                   .presentMode = VK_PRESENT_MODE_FIFO_KHR,
                                   .clipped = VK_TRUE};
    VkSwapchainKHR chain;
    double t0 = now_ns();
    VK(vkCreateSwapchainKHR(device, &sc, NULL, &chain));
    printf("timing swapchain creation %.0f us\n", (now_ns() - t0) / 1e3);
    uint32_t n = 0;
    VK(vkGetSwapchainImagesKHR(device, chain, &n, NULL));
    VkImage images[8];
    CHECK(n <= 8, "%u swapchain images", n);
    VK(vkGetSwapchainImagesKHR(device, chain, &n, images));
    VkImageView views[8];
    VkFramebuffer fbs[8];
    VkCommandBuffer cmds[8];
    for (uint32_t i = 0; i < n; i++) {
        views[i] = view_of(images[i]);
        fbs[i] = framebuffer(views[i]);
        cmds[i] = record(fbs[i], 0.0f, 1.0f, 0.0f);
    }
    VkSemaphoreCreateInfo sem = {.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO};
    VkSemaphore acquired, rendered;
    VK(vkCreateSemaphore(device, &sem, NULL, &acquired));
    VK(vkCreateSemaphore(device, &sem, NULL, &rendered));
    const int frames = 100;
    t0 = now_ns();
    for (int f = 0; f < frames; f++) {
        uint32_t index;
        VK(vkAcquireNextImageKHR(device, chain, UINT64_MAX, acquired, VK_NULL_HANDLE, &index));
        VkPipelineStageFlags stage = VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        VkSubmitInfo si2 = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
                            .waitSemaphoreCount = 1,
                            .pWaitSemaphores = &acquired,
                            .pWaitDstStageMask = &stage,
                            .commandBufferCount = 1,
                            .pCommandBuffers = &cmds[index],
                            .signalSemaphoreCount = 1,
                            .pSignalSemaphores = &rendered};
        VK(vkQueueSubmit(queue, 1, &si2, VK_NULL_HANDLE));
        VkPresentInfoKHR pi = {.sType = VK_STRUCTURE_TYPE_PRESENT_INFO_KHR,
                               .waitSemaphoreCount = 1,
                               .pWaitSemaphores = &rendered,
                               .swapchainCount = 1,
                               .pSwapchains = &chain,
                               .pImageIndices = &index};
        VK(vkQueuePresentKHR(queue, &pi));
        AImage* image = NULL;
        if (f == frames - 1) {
            CHECK(AImageReader_acquireLatestImage(reader, &image) == AMEDIA_OK, "acquire image");
            AHardwareBuffer* queued = NULL;
            CHECK(AImage_getHardwareBuffer(image, &queued) == AMEDIA_OK, "image buffer");
            check_frame(queued, 0x00ff00ff, "swapchain image");
            AImage_delete(image);
        } else if (AImageReader_acquireLatestImage(reader, &image) == AMEDIA_OK) {
            AImage_delete(image);
        }
    }
    printf("timing swapchain frame (acquire + submit + present + reader acquire) %.1f us\n",
           (now_ns() - t0) / frames / 1e3);
    VK(vkQueueWaitIdle(queue));
    for (uint32_t i = 0; i < n; i++) {
        vkFreeCommandBuffers(device, pool, 1, &cmds[i]);
        vkDestroyFramebuffer(device, fbs[i], NULL);
        vkDestroyImageView(device, views[i], NULL);
    }
    vkDestroySemaphore(device, acquired, NULL);
    vkDestroySemaphore(device, rendered, NULL);
    vkDestroySwapchainKHR(device, chain, NULL);
    vkDestroySurfaceKHR(instance, surface, NULL);
    AImageReader_delete(reader);
    printf("ok swapchain on an ImageReader window: %u images, %d frames\n", n, frames);
}

int main(int argc, char** argv) {
    // Line by line, so a hang shows how far it got.
    setvbuf(stdout, NULL, _IOLBF, 0);
    if (argc > 1 && !strcmp(argv[1], "info")) return info();
    double t0 = now_ns();
    create_instance();
    VkPhysicalDeviceProperties p;
    vkGetPhysicalDeviceProperties(gpu, &p);
    printf("ok instance: %s, Vulkan %u.%u (%.0f us)\n", p.deviceName,
           VK_VERSION_MAJOR(p.apiVersion), VK_VERSION_MINOR(p.apiVersion),
           (now_ns() - t0) / 1e3);

    uint32_t n = 0;
    vkEnumerateDeviceExtensionProperties(gpu, NULL, &n, NULL);
    VkExtensionProperties* e = calloc(n, sizeof(*e));
    vkEnumerateDeviceExtensionProperties(gpu, NULL, &n, e);
    const char* exts[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME,
                          VK_ANDROID_EXTERNAL_MEMORY_ANDROID_HARDWARE_BUFFER_EXTENSION_NAME,
                          VK_EXT_QUEUE_FAMILY_FOREIGN_EXTENSION_NAME,
                          VK_KHR_EXTERNAL_SEMAPHORE_FD_EXTENSION_NAME};
    for (int i = 0; i < 4; i++) CHECK(has(e, n, exts[i]), "no %s", exts[i]);
    free(e);
    // As HWUI's VulkanManager: two queues of the first graphics family.
    uint32_t families = 0;
    vkGetPhysicalDeviceQueueFamilyProperties(gpu, &families, NULL);
    VkQueueFamilyProperties* fp = calloc(families, sizeof(*fp));
    vkGetPhysicalDeviceQueueFamilyProperties(gpu, &families, fp);
    uint32_t family = 0;
    while (family < families && !(fp[family].queueFlags & VK_QUEUE_GRAPHICS_BIT)) family++;
    CHECK(family < families && fp[family].queueCount >= 2,
          "graphics family %u of %u has %u queues", family, families,
          family < families ? fp[family].queueCount : 0);
    printf("ok %u queue family, graphics family %u with %u queues\n", families, family,
           fp[family].queueCount);
    free(fp);
    float priorities[2] = {1.0f, 1.0f};
    VkDeviceQueueCreateInfo q = {.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
                                 .queueFamilyIndex = family,
                                 .queueCount = 2,
                                 .pQueuePriorities = priorities};
    VkPhysicalDeviceTimelineSemaphoreFeatures timelines = {
        .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_TIMELINE_SEMAPHORE_FEATURES,
        .timelineSemaphore = VK_TRUE};
    VkDeviceCreateInfo di = {.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
                             .pNext = &timelines,
                             .queueCreateInfoCount = 1,
                             .pQueueCreateInfos = &q,
                             .enabledExtensionCount = 4,
                             .ppEnabledExtensionNames = exts};
    VK(vkCreateDevice(gpu, &di, NULL, &device));
    vkGetDeviceQueue(device, family, 0, &queue);
    vkGetDeviceQueue(device, family, 1, &queue2);
    CHECK(queue && queue2 && queue != queue2, "queues %p %p", (void*)queue, (void*)queue2);
    VkCommandPoolCreateInfo pi = {.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
                                  .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
                                  .queueFamilyIndex = family};
    VK(vkCreateCommandPool(device, &pi, NULL, &pool));
    printf("ok device with VK_KHR_swapchain, AHardwareBuffers and sync fds\n");

    if (argc > 1 && !strcmp(argv[1], "swapchain")) {
        create_pipeline(VK_IMAGE_LAYOUT_PRESENT_SRC_KHR);
        swapchain();
    } else {
        create_pipeline(VK_IMAGE_LAYOUT_GENERAL);
        ahb_triangle();
        sync_fd();
    }
    vkDestroyPipeline(device, pipeline, NULL);
    vkDestroyRenderPass(device, pass, NULL);
    vkDestroyPipelineLayout(device, layout, NULL);
    vkDestroyCommandPool(device, pool, NULL);
    vkDestroyDevice(device, NULL);
    vkDestroyInstance(instance, NULL);
    printf("ok done\n");
    return 0;
}

