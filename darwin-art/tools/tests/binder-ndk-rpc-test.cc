#include <android/binder_ibinder.h>
#include <android/binder_libbinder.h>
#include <android/binder_parcel.h>
#include <binder/RpcServer.h>
#include <binder/RpcSession.h>
#include <atomic>
#include <cassert>
#include <cstdio>
#include <fcntl.h>
#include <sys/socket.h>
#include <unistd.h>

struct State { std::atomic<int> calls{0}, destroyed{0}; };
static void* create(void* arg) { return arg; }
static void destroy(void* arg) { ++static_cast<State*>(arg)->destroyed; }
static binder_status_t transact(AIBinder* binder, transaction_code_t code,
                                 const AParcel* in, AParcel* out) {
    if (code != FIRST_CALL_TRANSACTION) return STATUS_UNKNOWN_TRANSACTION;
    int32_t value = 0;
    int fd = -1;
    if (AParcel_readInt32(in, &value) != STATUS_OK) return STATUS_BAD_VALUE;
    if (AParcel_readParcelFileDescriptor(in, &fd) != STATUS_OK) return STATUS_BAD_VALUE;
    assert(fd >= 0);
    assert(AParcel_writeInt32(out, value + 1) == STATUS_OK);
    assert(AParcel_writeParcelFileDescriptor(out, fd) == STATUS_OK);
    close(fd);
    ++static_cast<State*>(AIBinder_getUserData(binder))->calls;
    return STATUS_OK;
}

int main() {
    // Bound only this test process, never any running Android app.
    alarm(20);
    using namespace android;
    State state;
    AIBinder_Class* clazz = AIBinder_Class_define("darwin.tests.INdkRpc", create, destroy, transact);
    AIBinder* local = AIBinder_new(clazz, &state);
    assert(local && !AIBinder_isRemote(local));
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
    for (int fd : sockets) assert(fcntl(fd, F_SETFD, FD_CLOEXEC) == 0);
    auto server = RpcServer::make();
    server->setMaxThreads(2);
    server->setSupportedFileDescriptorTransportModes({RpcSession::FileDescriptorTransportMode::UNIX});
    server->setRootObject(AIBinder_toPlatformBinder(local));
    assert(server->setupUnixDomainSocketBootstrapServer(binder::unique_fd(sockets[0])) == OK);
    server->start();
    auto session = RpcSession::make();
    session->setFileDescriptorTransportMode(RpcSession::FileDescriptorTransportMode::UNIX);
    assert(session->setupUnixDomainSocketBootstrapClient(binder::unique_fd(sockets[1])) == OK);
    AIBinder* remote = AIBinder_fromPlatformBinder(session->getRootObject());
    assert(remote && AIBinder_isRemote(remote));
    assert(AIBinder_associateClass(remote, clazz));
    for (int i = 0; i < 16; ++i) {
        int pipe_fds[2];
        assert(pipe(pipe_fds) == 0);
        AParcel *in = nullptr, *out = nullptr;
        assert(AIBinder_prepareTransaction(remote, &in) == STATUS_OK);
        assert(AParcel_writeInt32(in, i) == STATUS_OK);
        assert(AParcel_writeParcelFileDescriptor(in, pipe_fds[0]) == STATUS_OK);
        close(pipe_fds[0]);
        assert(AIBinder_transact(remote, FIRST_CALL_TRANSACTION, &in, &out, 0) == STATUS_OK);
        assert(in == nullptr && out);
        int32_t reply = 0;
        int returned_fd = -1;
        assert(AParcel_readInt32(out, &reply) == STATUS_OK && reply == i + 1);
        assert(AParcel_readParcelFileDescriptor(out, &returned_fd) == STATUS_OK);
        AParcel_delete(out);
        assert((fcntl(returned_fd, F_GETFD) & FD_CLOEXEC) != 0);
        assert(write(pipe_fds[1], "x", 1) == 1);
        char value = 0;
        assert(read(returned_fd, &value, 1) == 1 && value == 'x');
        close(returned_fd);
        close(pipe_fds[1]);
    }
    assert(state.calls == 16);
    AIBinder_decStrong(remote);
    assert(session->shutdownAndWait(true));
    assert(server->shutdown());
    server->setRootObject(nullptr);
    session.clear();
    server.clear();
    AIBinder_decStrong(local);
    assert(state.destroyed == 1);
    alarm(0);
    std::puts("original NDK Binder RPC: 16 transactions, Parcel ownership, FD roundtrip/CLOEXEC, destruction PASS");
}
