#pragma once

#include <android-base/unique_fd.h>
#include <cstddef>
#include <vector>

namespace darwin_art::release_record {

// Fixed-size libgui release records only, not a general SOCK_SEQPACKET shim.
// Endpoints remain owned by AOSP and can be transferred as ordinary socket FDs.
int open(int endpoints[2]);

// Caller serializes writers, including writes through duplicated endpoints.
// An incomplete send failure poisons the connection, never the next record.
int send(int socket, const void* bytes, size_t size, int descriptor);

// Owned by the Android ConsumerEndpoint under its existing read mutex. Drains
// partial bytes so readiness does not spin while waiting for the rest of a record.
class Receiver {
public:
    explicit Receiver(size_t recordSize) : pending_(recordSize) {}
    int receive(int socket, void* bytes, size_t size, android::base::unique_fd& descriptor);

private:
    int fail(int error);
    std::vector<unsigned char> pending_;
    size_t used_ = 0;
    android::base::unique_fd descriptor_;
    bool failed_ = false;
};

} // namespace darwin_art::release_record
