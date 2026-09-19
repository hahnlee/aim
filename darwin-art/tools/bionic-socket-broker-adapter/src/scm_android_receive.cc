#include "scm_android_receive.h"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstring>
#include <cstdlib>
#include <unistd.h>

namespace darwin_art::bionic::scm::android {
namespace {

constexpr int kAndroidSolSocket = 1;
constexpr int kAndroidScmRights = 1;
constexpr int kAndroidScmCredentials = 2;
constexpr int kAndroidMsgOob = 0x1;
constexpr int kAndroidMsgEor = 0x80;
constexpr int kAndroidMsgTrunc = 0x20;
constexpr int kAndroidMsgCtrunc = 0x8;
constexpr std::size_t kMaxPayloads = DARWIN_ART_SCM_MAX_PAYLOADS;

struct AndroidCmsghdr {
  std::size_t length;
  int32_t level;
  int32_t type;
};

static_assert(sizeof(AndroidCmsghdr) == 16);

int AndroidError(const Callbacks &callbacks, int error) noexcept {
  if (callbacks.android_errno != nullptr) {
    const int translated = callbacks.android_errno(error);
    return translated != 0 ? translated : 5;
  }
  return error != 0 ? error : 5;
}

int CallbackError(const Callbacks &callbacks, int error) noexcept {
  return error != 0 ? error : AndroidError(callbacks, EINVAL);
}

void ReleasePrepared(const Callbacks &callbacks,
                     const PreparedGuestDescriptor &prepared) noexcept {
  if (prepared.central.object != 0 && callbacks.release_central != nullptr)
    callbacks.release_central(callbacks.context, prepared.central.object);
  if (prepared.filesystem_fd >= 0) (void)::close(prepared.filesystem_fd);
}

std::size_t AlignControl(std::size_t length) noexcept {
  constexpr std::size_t alignment = sizeof(std::size_t);
  return (length + alignment - 1) & ~(alignment - 1);
}

}  // namespace

int ReceiveManagedMessage(SCMChannel &channel, const NativeMessage &message,
                          const ReceiveOptions &options,
                          GuestDescriptorGroup &group,
                          const ReceiveRequest &request,
                          const Callbacks &callbacks, ReceiveOutput *output,
                          ssize_t *bytes) noexcept {
  if (output == nullptr || bytes == nullptr || callbacks.context == nullptr ||
      callbacks.prepare_payload == nullptr || callbacks.release_central == nullptr ||
      callbacks.android_errno == nullptr ||
      (request.control.capacity != 0 && request.control.data == nullptr) ||
      (request.name.capacity != 0 && request.name.data == nullptr))
    return AndroidError(callbacks, EINVAL);
  ReceiveOutput staged_output{};

  sockaddr_storage host_name{};
  NativeMessage receive_message = message;
  if (request.name.data != nullptr) {
    receive_message.name = &host_name;
    receive_message.name_length = sizeof(host_name);
  } else {
    receive_message.name = nullptr;
    receive_message.name_length = 0;
  }

  ReceiveResult received;
  if (channel.Receive(receive_message, options, &received) != 0)
    return AndroidError(callbacks, errno);
  VerifiedPeerCredentials authenticated_credentials{};

  const std::size_t selected =
      received.private_envelope() &&
              request.control.capacity >= sizeof(AndroidCmsghdr) + sizeof(int)
          ? std::min(received.payload_count(),
                     (request.control.capacity - sizeof(AndroidCmsghdr)) /
                         sizeof(int))
          : 0;
  std::array<uint64_t, kMaxPayloads> ordinals{};
  for (std::size_t i = 0; i < selected; ++i) ordinals[i] = i;
  std::array<GrantLease, kMaxPayloads> grants{};

  if (received.private_envelope()) {
    if (received.Admit(ordinals.data(), selected) != 0)
      return AndroidError(callbacks, errno);
    if (received.Finish() != 0) return AndroidError(callbacks, errno);
    if (request.credentials != nullptr) {
      const ScmCredentials *credentials = received.credentials();
      if (credentials == nullptr) return AndroidError(callbacks, EPROTO);
      authenticated_credentials = {credentials->process_id, credentials->user_id,
                                   credentials->group_id};
    }
    for (std::size_t i = 0; i < received.claim_count(); ++i) {
      const uint64_t ordinal = received.claim_ordinal(i);
      if (ordinal >= selected || !received.TakeGrant(i, &grants[ordinal]))
        return AndroidError(callbacks, EINVAL);
    }
    for (std::size_t i = 0; i < received.payload_count(); ++i) {
      if (i >= selected) {
        if (!received.DiscardPayloadFd(i))
          return AndroidError(callbacks, errno);
        continue;
      }
      int native = -1;
      if (!received.TakePayloadFd(i, &native))
        return AndroidError(callbacks, errno);
      PreparedGuestDescriptor prepared{};
      prepared.filesystem_fd = -1;
      GrantLease *grant = grants[i].valid() ? &grants[i] : nullptr;
      const int error = callbacks.prepare_payload(
          callbacks.context, native, grant, request.descriptor_flags, &prepared);
      if (error != 0) return CallbackError(callbacks, error);
      if (!group.Add(prepared)) {
        ReleasePrepared(callbacks, prepared);
        return AndroidError(callbacks, EINVAL);
      }
    }
    if (!received.ReadyToCommit()) return AndroidError(callbacks, EINVAL);
  }
  if (request.credentials != nullptr && !received.private_envelope())
    return AndroidError(callbacks, EPROTO);

  if (request.name.data != nullptr && received.name_length() != 0) {
    if (callbacks.decode_address == nullptr ||
        !callbacks.decode_address(callbacks.context,
                                  reinterpret_cast<const sockaddr *>(&host_name),
                                  received.name_length(), request.name.data,
                                  request.name.capacity,
                                  &staged_output.name_length))
      return AndroidError(callbacks, EAFNOSUPPORT);
  }

  const int publication_error = group.Publish();
  if (publication_error != 0) return publication_error;
  if (received.private_envelope() && !received.Commit()) std::abort();

  staged_output.flags = 0;
  if ((received.flags() & MSG_OOB) != 0) staged_output.flags |= kAndroidMsgOob;
  if ((received.flags() & MSG_EOR) != 0) staged_output.flags |= kAndroidMsgEor;
  if ((received.flags() & MSG_TRUNC) != 0) staged_output.flags |= kAndroidMsgTrunc;
  if ((received.flags() & MSG_CTRUNC) != 0 || selected < received.payload_count())
    staged_output.flags |= kAndroidMsgCtrunc;

  if (selected != 0) {
    const std::size_t length = sizeof(AndroidCmsghdr) + selected * sizeof(int);
    const AndroidCmsghdr header{length, kAndroidSolSocket, kAndroidScmRights};
    std::memcpy(request.control.data, &header, sizeof(header));
    for (std::size_t i = 0; i < selected; ++i) {
      const int guest = group.guest_fd(i);
      std::memcpy(static_cast<uint8_t *>(request.control.data) +
                      sizeof(header) + i * sizeof(int),
                  &guest, sizeof(guest));
    }
    staged_output.control_length =
        std::min(request.control.capacity, AlignControl(length));
  }
  if (request.credentials != nullptr) {
    const std::size_t length = sizeof(AndroidCmsghdr) +
                               sizeof(VerifiedPeerCredentials);
    const std::size_t space = AlignControl(length);
    if (request.control.capacity - staged_output.control_length >= space) {
      const AndroidCmsghdr header{length, kAndroidSolSocket,
                                  kAndroidScmCredentials};
      auto *base = static_cast<uint8_t *>(request.control.data) +
                   staged_output.control_length;
      std::memcpy(base, &header, sizeof(header));
      std::memcpy(base + sizeof(header), &authenticated_credentials,
                  sizeof(authenticated_credentials));
      staged_output.control_length += space;
    } else {
      staged_output.flags |= kAndroidMsgCtrunc;
    }
  }
  *output = staged_output;
  *bytes = received.bytes();
  return 0;
}

}  // namespace darwin_art::bionic::scm::android
