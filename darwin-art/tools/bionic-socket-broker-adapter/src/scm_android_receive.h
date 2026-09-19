#ifndef DARWIN_ART_SCM_ANDROID_RECEIVE_H_
#define DARWIN_ART_SCM_ANDROID_RECEIVE_H_

#include "scm_channel.h"
#include "scm_guest_group.h"

#include <cstddef>
#include <cstdint>
#include <sys/socket.h>
#include <sys/types.h>

namespace darwin_art::bionic::scm::android {

struct NameView {
  void *data = nullptr;
  uint32_t capacity = 0;
};

struct ControlView {
  void *data = nullptr;
  std::size_t capacity = 0;
};

struct VerifiedPeerCredentials {
  int32_t process_id = 0;
  uint32_t user_id = 0;
  uint32_t group_id = 0;
};

struct ReceiveRequest {
  NameView name;
  ControlView control;
  int descriptor_flags = 0;
  const VerifiedPeerCredentials *credentials = nullptr;
};

struct ReceiveOutput {
  std::size_t control_length = 0;
  uint32_t name_length = 0;
  int flags = 0;
};

using PreparePayload = int (*)(void *, int, GrantLease *, int,
                               PreparedGuestDescriptor *);
using ReleaseCentral = void (*)(void *, uint64_t);
using DecodeAddress = bool (*)(void *, const sockaddr *, socklen_t, void *,
                               uint32_t, uint32_t *);
using AndroidErrno = int (*)(int);

struct Callbacks {
  void *context = nullptr;
  PreparePayload prepare_payload = nullptr;
  ReleaseCentral release_central = nullptr;
  DecodeAddress decode_address = nullptr;
  AndroidErrno android_errno = nullptr;
};

// Consume one native message from a borrowed managed SCMChannel. The caller
// owns the channel, exact endpoint Description, and GuestDescriptorGroup; this
// function owns the ReceiveResult and all grant/native-right handoffs until
// the group has published and the result has committed.
int ReceiveManagedMessage(SCMChannel &channel, const NativeMessage &message,
                          const ReceiveOptions &options,
                          GuestDescriptorGroup &group,
                          const ReceiveRequest &request,
                          const Callbacks &callbacks, ReceiveOutput *output,
                          ssize_t *bytes) noexcept;

}  // namespace darwin_art::bionic::scm::android

#endif  // DARWIN_ART_SCM_ANDROID_RECEIVE_H_
