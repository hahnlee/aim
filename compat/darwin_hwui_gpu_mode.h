#ifndef AIM_COMPAT_HWUI_GPU_MODE_H_
#define AIM_COMPAT_HWUI_GPU_MODE_H_

namespace aim {

// GPU mode is the Darwin production path.  A CPU build is available only for
// source/registrar diagnostics; an application launcher must not silently
// downgrade when Metal is unavailable.
inline bool hwui_gpu_enabled() {
#if defined(AIM_HWUI_GPU)
    // Production graphics is intentionally GPU-only.  CPU rendering is a
    // separate registrar/layout diagnostic build and is never selected by an
    // application environment variable.
    return true;
#else
    return false;
#endif
}

}  // namespace aim

#endif  // AIM_COMPAT_HWUI_GPU_MODE_H_
