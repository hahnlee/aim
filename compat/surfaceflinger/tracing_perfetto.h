#pragma once

#include <cstdint>

// SurfaceFlinger's transaction/composition semantics do not depend on tracing.
// Darwin keeps the call sites intact while the Perfetto transport is absent.
namespace tracing_perfetto {
inline bool isTagEnabled(std::uint64_t) { return false; }
template <typename... Args> inline void traceBegin(Args&&...) {}
template <typename... Args> inline void traceEnd(Args&&...) {}
template <typename... Args> inline void traceAsyncBegin(Args&&...) {}
template <typename... Args> inline void traceAsyncEnd(Args&&...) {}
template <typename... Args> inline void traceAsyncBeginForTrack(Args&&...) {}
template <typename... Args> inline void traceAsyncEndForTrack(Args&&...) {}
template <typename... Args> inline void traceInstant(Args&&...) {}
template <typename... Args> inline void traceFormatInstant(Args&&...) {}
template <typename... Args> inline void traceInstantForTrack(Args&&...) {}
template <typename... Args> inline void traceCounter32(Args&&...) {}
template <typename... Args> inline void traceCounter(Args&&...) {}
template <typename... Args> inline void traceFormatBegin(Args&&...) {}
}  // namespace tracing_perfetto
