#pragma once

namespace android::GpuStatsInfo {
// EGL_CONTEXT_OPENGL_BACKWARDS_COMPATIBLE_ANGLE is followed by this Android
// telemetry hint.  ANGLE ignores the hint when telemetry is not available.
inline constexpr int SKIP_TELEMETRY = 0x3482;
}  // namespace android::GpuStatsInfo
