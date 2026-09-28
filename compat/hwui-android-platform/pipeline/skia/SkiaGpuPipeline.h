#pragma once

// Darwin uses Android's production HWUI GPU pipeline.  Only the scheduler and
// device-facing services are Darwin backends; selecting platform/host here
// silently replaces rendering with the null-GPU test implementation.
#include "platform/android/pipeline/skia/SkiaGpuPipeline.h"
