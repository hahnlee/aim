// Tests actual native provider ownership, never a daemon pair-registration ACK.
#include "scm_endpoint_provider.h"
#include <cassert>
#include <cerrno>

namespace {
struct Context {
  int references = 1;
  bool reenter = false;
  DarwinArtScmEndpointProviderV1 table{};
};
void *Retain(void *opaque) {
  auto &context = *static_cast<Context *>(opaque);
  ++context.references;
  if (context.reenter) {
    assert(darwin_art_bionic_install_scm_endpoint_provider(&context.table) == EALREADY);
    assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  }
  return opaque;
}
void Release(void *opaque) {
  auto &context = *static_cast<Context *>(opaque);
  if (context.reenter) {
    assert(darwin_art_bionic_install_scm_endpoint_provider(&context.table) == EALREADY);
    assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  }
  --context.references;
}
int Register(void *, const DarwinArtScmPairInstallerV1 *) {
  // In-flight delegate cannot uninstall its own executing code.
  assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  return ENOSYS;
}
int ReleaseHolder(void *, const uint8_t *) {
  assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  return ENOSYS;
}
} // namespace

int main() {
  using darwin_art::bionic::scm::AcquireProvider;
  Context context;
  context.table = {DARWIN_ART_SCM_ENDPOINT_ABI_VERSION, sizeof(context.table),
                   &context, &Retain, &Release, &Register, &ReleaseHolder};
  DarwinArtScmEndpointProviderV1 acquired{};
  assert(AcquireProvider(&acquired) == ENOSYS);
  assert(darwin_art_bionic_install_scm_endpoint_provider(nullptr) == EINVAL);
  auto invalid = context.table;
  invalid.struct_size = 0;
  assert(darwin_art_bionic_install_scm_endpoint_provider(&invalid) == EINVAL);
  context.reenter = true;
  auto caller_table = context.table;
  assert(darwin_art_bionic_install_scm_endpoint_provider(&caller_table) == 0);
  assert(context.references == 2);
  caller_table = {}; // The installed table must have copied its callbacks.
  assert(darwin_art_bionic_install_scm_endpoint_provider(&context.table) == EALREADY);
  assert(context.references == 2);
  assert(AcquireProvider(&acquired) == 0);
  assert(acquired.register_pair(acquired.context, nullptr) == ENOSYS);
  assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == EBUSY);
  DarwinArtScmEndpointProviderV1 stopped{};
  assert(AcquireProvider(&stopped) == ENOSYS);
  void *existing = acquired.retain(acquired.context);
  assert(existing != nullptr);
  uint8_t holder[16]{};
  assert(acquired.release_holder(existing, holder) == ENOSYS);
  acquired.release(existing);
  acquired.release(acquired.context);
  assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == 0);
  assert(context.references == 1);
  assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == 0);
  // A fresh installation is legal only after retirement completes.
  assert(darwin_art_bionic_install_scm_endpoint_provider(&context.table) == 0);
  assert(darwin_art_bionic_uninstall_scm_endpoint_provider() == 0);
  assert(context.references == 1);
}
