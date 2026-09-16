#pragma once
#include <sys/types.h>

namespace darwin_art::binder {
struct PeerCredentials {
  pid_t pid;
  uid_t host_uid;
  gid_t host_gid;
};

// Kernel-authenticated connection credentials, not values from Parcel data.
// A transferred/pre-created socket identifies its original peer, not whoever
// later writes it. Launch-inherited channels require process-registry binding.
bool ReadPeerCredentials(int socket, PeerCredentials* out);
}
