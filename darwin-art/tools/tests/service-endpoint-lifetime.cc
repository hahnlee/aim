// Native ownership contract fixture; no profile or Android runtime is started.
#include "../../compat/process/service_endpoint.h"
#include <cassert>

namespace {
int ready_status = 0;
int loss_status = 0;
int closes = 0;
int losses = 0;
uint32_t lost_bits = 0;
}
extern "C" int32_t darwin_art_service_endpoint_open(
    const uint8_t*, size_t, uint64_t* handle, int32_t* descriptor) {
  *handle = 1;
  *descriptor = 17;
  return 0;
}
extern "C" int32_t darwin_art_service_endpoint_close(uint64_t handle) {
  assert(handle == 1);
  ++closes;
  return 0;
}
extern "C" int32_t darwin_art_service_endpoint_ready(uint64_t, uint32_t) {
  return ready_status;
}
extern "C" int32_t darwin_art_service_endpoint_lost(uint64_t, uint32_t bits) {
  ++losses;
  lost_bits = bits;
  return loss_status;
}

int main() {
  using namespace darwin_art::process;
  {
    ServiceEndpoint original("/fixture");
    assert(original.publish_ready(ServiceReadiness::Binder) == ReadinessPublication::Published);
    ServiceEndpoint moved(std::move(original));
    assert(!original);
    assert(moved.publish_ready(ServiceReadiness::Compositor) == ReadinessPublication::Published);
  }
  assert(closes == 1 && losses == 1 && lost_bits == 3);
  {
    ServiceEndpoint listener("/fixture");
    listener.publish_ready(ServiceReadiness::Binder);
    assert(listener.report_lost(ServiceReadiness::Binder) == ReadinessPublication::Published);
  }
  assert(closes == 2 && losses == 2 && lost_bits == 1);
  {
    ServiceEndpoint listener("/fixture");
    ready_status = 1;
    assert(listener.publish_ready(ServiceReadiness::Binder) == ReadinessPublication::Unmanaged);
  }
  assert(closes == 3 && losses == 2);
  {
    ServiceEndpoint listener("/fixture");
    ready_status = 0;
    listener.publish_ready(ServiceReadiness::Binder);
    loss_status = -1;
    assert(listener.report_lost(ServiceReadiness::Binder) == ReadinessPublication::Failed);
    loss_status = 0;
  }
  assert(closes == 4 && losses == 4); // Failed report retried on owned close.
}
