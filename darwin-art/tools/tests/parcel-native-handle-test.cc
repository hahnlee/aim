#include <binder/Parcel.h>
#include <binder/RpcSession.h>
#include <cutils/native_handle.h>
#include <cassert>
#include <cstdio>
#include <fcntl.h>
#include <unistd.h>

int main() {
    using namespace android;
    auto session = RpcSession::make();
    assert(session);
    session->setFileDescriptorTransportMode(RpcSession::FileDescriptorTransportMode::UNIX);
    int pipeFds[2];
    assert(pipe(pipeFds) == 0);
    native_handle_t* source = native_handle_create(1, 2);
    assert(source);
    source->data[0] = pipeFds[0];
    source->data[1] = 123;
    source->data[2] = -456;
    native_handle_t* decoded = nullptr;
    {
        Parcel parcel;
        parcel.markForRpc(session);
        assert(parcel.writeNativeHandle(source) == NO_ERROR);
        assert(native_handle_close(source) == 0);
        assert(native_handle_delete(source) == 0);
        parcel.setDataPosition(0);
        decoded = parcel.readNativeHandle();
        assert(decoded && decoded->numFds == 1 && decoded->numInts == 2);
        assert(decoded->data[1] == 123 && decoded->data[2] == -456);
        assert(fcntl(decoded->data[0], F_GETFD) & FD_CLOEXEC);
    }
    // Source handle and Parcel have both released their descriptors. The decoded
    // native handle must still own an independent live FD to the same pipe.
    assert(write(pipeFds[1], "Q", 1) == 1);
    char value = 0;
    assert(read(decoded->data[0], &value, 1) == 1 && value == 'Q');
    assert(native_handle_close(decoded) == 0);
    assert(native_handle_delete(decoded) == 0);
    close(pipeFds[1]);
    // libgui and Binder must share the original libbase unique_fd ABI.
    assert(pipe(pipeFds) == 0);
    base::unique_fd uniqueDecoded;
    {
        base::unique_fd uniqueSource(pipeFds[0]);
        Parcel parcel;
        parcel.markForRpc(session);
        assert(parcel.writeUniqueFileDescriptor(uniqueSource) == NO_ERROR);
        uniqueSource.reset();
        parcel.setDataPosition(0);
        assert(parcel.readUniqueFileDescriptor(&uniqueDecoded) == NO_ERROR);
        assert(uniqueDecoded.ok());
        assert(fcntl(uniqueDecoded.get(), F_GETFD) & FD_CLOEXEC);
    }
    assert(write(pipeFds[1], "U", 1) == 1);
    assert(read(uniqueDecoded.get(), &value, 1) == 1 && value == 'U');
    uniqueDecoded.reset();
    close(pipeFds[1]);
    for (int count : {-1, 2000000}) {
        Parcel invalid;
        invalid.markForRpc(session);
        assert(invalid.writeInt32(count) == NO_ERROR);
        assert(invalid.writeInt32(0) == NO_ERROR);
        invalid.setDataPosition(0);
        assert(!invalid.readNativeHandle());
    }
    Parcel truncated;
    truncated.markForRpc(session);
    assert(truncated.writeInt32(0) == NO_ERROR);
    assert(truncated.writeInt32(1) == NO_ERROR);
    truncated.setDataPosition(0);
    assert(!truncated.readNativeHandle());
    std::puts("original Parcel native handle/unique_fd: UNIX-RPC FD ownership/CLOEXEC, integer payload, invalid counts and truncation PASS");
}
