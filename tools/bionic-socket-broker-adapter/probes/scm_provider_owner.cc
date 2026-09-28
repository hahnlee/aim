// Tests actual native provider ownership, never a daemon pair-registration ACK.
#include "scm_endpoint_provider.h"
#include <cassert>
#include <cerrno>

namespace {
struct Context {
  int references = 1;
  bool reenter = false;
  AimScmEndpointProviderV1 table{};
};
void *Retain(void *opaque) {
  auto &context = *static_cast<Context *>(opaque);
  ++context.references;
  if (context.reenter) {
    assert(aim_bionic_install_scm_endpoint_provider(&context.table) == EALREADY);
    assert(aim_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  }
  return opaque;
}
void Release(void *opaque) {
  auto &context = *static_cast<Context *>(opaque);
  if (context.reenter) {
    assert(aim_bionic_install_scm_endpoint_provider(&context.table) == EALREADY);
    assert(aim_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  }
  --context.references;
}
int Register(void *, const AimScmPairInstallerV1 *) {
  // In-flight delegate cannot uninstall its own executing code.
  assert(aim_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  return ENOSYS;
}
int ReleaseHolder(void *, const uint8_t *) {
  assert(aim_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  return ENOSYS;
}
int Prepare(void *, const AimScmPrepareRequestV2 *, AimScmPreparedV2 *) { return ENOSYS; }
int Admit(void *, const AimScmAdmitRequestV2 *, AimScmAdmissionV2 *) { return ENOSYS; }
int Settle(void *, const uint8_t *, uint64_t, uint32_t) {
  assert(aim_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  return ENOSYS;
}
int Bind(void *, const uint8_t *, const AimScmBinderBindingV2 *, uint8_t *) { return ENOSYS; }
int Cancel(void *, const AimScmBinderBindingV2 *) {
  assert(aim_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  return ENOSYS;
}
int Claim(void *, const AimScmBinderBindingV2 *, const uint8_t *, uint32_t, AimScmGrantV2 *) { return ENOSYS; }
} // namespace

int main() {
  using aim::bionic::scm::AcquireProvider;
  Context context;
  context.table = {AIM_SCM_ENDPOINT_ABI_VERSION, sizeof(context.table),
                   &context, &Retain, &Release, &Register, &ReleaseHolder,
                   &Prepare, &Admit, &Settle, &Bind, &Cancel, &Claim};
  AimScmEndpointProviderV1 acquired{};
  assert(AcquireProvider(&acquired) == ENOSYS);
  assert(aim_bionic_install_scm_endpoint_provider(nullptr) == EINVAL);
  auto invalid = context.table;
  invalid.struct_size = 0;
  assert(aim_bionic_install_scm_endpoint_provider(&invalid) == EINVAL);
  context.reenter = true;
  auto caller_table = context.table;
  assert(aim_bionic_install_scm_endpoint_provider(&caller_table) == 0);
  assert(context.references == 2);
  caller_table = {}; // The installed table must have copied its callbacks.
  assert(aim_bionic_install_scm_endpoint_provider(&context.table) == EALREADY);
  assert(context.references == 2);
  assert(AcquireProvider(&acquired) == 0);
  assert(acquired.register_pair(acquired.context, nullptr) == ENOSYS);
  assert(aim_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  AimScmEndpointProviderV1 stopped{};
  assert(AcquireProvider(&stopped) == ENOSYS);
  assert(acquired.prepare(acquired.context, nullptr, nullptr) == ESHUTDOWN);
  assert(acquired.admit(acquired.context, nullptr, nullptr) == ESHUTDOWN);
  assert(acquired.bind_binder(acquired.context, nullptr, nullptr, nullptr) == ESHUTDOWN);
  assert(acquired.claim_binder(acquired.context, nullptr, nullptr, 0, nullptr) == ESHUTDOWN);
  assert(acquired.settle(acquired.context, nullptr, 0, AIM_SCM_ABORTED) == ENOSYS);
  assert(acquired.cancel_binder(acquired.context, nullptr) == ENOSYS);
  void *existing = acquired.retain(acquired.context);
  assert(existing != nullptr);
  uint8_t holder[16]{};
  assert(acquired.release_holder(existing, holder) == ENOSYS);
  acquired.release(existing);
  acquired.release(acquired.context);
  assert(aim_bionic_uninstall_scm_endpoint_provider() == 0);
  assert(context.references == 1);
  assert(aim_bionic_uninstall_scm_endpoint_provider() == 0);
  // A fresh installation is legal only after retirement completes.
  assert(aim_bionic_install_scm_endpoint_provider(&context.table) == 0);
  assert(aim_bionic_uninstall_scm_endpoint_provider() == 0);
  assert(context.references == 1);
}
