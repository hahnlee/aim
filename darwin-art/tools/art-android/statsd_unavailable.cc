// Build-only replacement for art/runtime/metrics/statsd.cc.
//
// statsd.cc needs statslog_art.{h,cpp}, which Soong generates with
// stats-log-api-gen from frameworks/proto_logging atoms. That generator is not
// built here yet (hahnlee/aim#162). These
// definitions match upstream's own non-Android inline versions in
// metrics/statsd.h: no statsd backend, no device-status callback.

#include "metrics/statsd.h"

#include "base/metrics/metrics.h"

namespace art HIDDEN {
namespace metrics {

std::unique_ptr<MetricsBackend> CreateStatsdBackend() { return nullptr; }
void SetupCallbackForDeviceStatus() {}
void ReportDeviceMetrics() {}

}  // namespace metrics
}  // namespace art
