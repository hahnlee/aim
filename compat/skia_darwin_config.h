#pragma once

// The AOSP Skia tree carries Android framework defaults in its generated
// SkUserConfig.h. The Darwin CPU build deliberately uses a small host config
// instead of inheriting Android logging and GPU policy.
#ifndef SK_R32_SHIFT
#define SK_R32_SHIFT 16
#endif
