#pragma once

// Keep the Android type contract visible even though the Darwin runtime
// advertises Vulkan as unavailable and selects SkiaGL.
#include "platform/android/pipeline/skia/SkiaVulkanPipeline.h"
