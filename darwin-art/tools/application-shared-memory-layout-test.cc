// Exercise the original AOSP implementation, not a duplicate layout.
#include "com_android_internal_os_ApplicationSharedMemory.cpp"
#include "../compat/memory/system_region.h"
#include <cassert>
#include <sys/wait.h>
#include <libproc.h>
#include <vector>
#include "tests/application-descriptor-table.h"

static int DescriptorBytes() {
    int count = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, nullptr, 0);
    assert(count > 0);
    std::vector<proc_fdinfo> entries(count / sizeof(proc_fdinfo) + 16);
    int actual = proc_pidinfo(getpid(), PROC_PIDLISTFDS, 0, entries.data(),
                             static_cast<int>(entries.size() * sizeof(proc_fdinfo)));
    assert(actual > 0 && actual < static_cast<int>(entries.size() * sizeof(proc_fdinfo)));
    return actual;
}

int main() {
    int fd = nativeCreate(nullptr, nullptr);
    assert(fd >= 0);
    auto* writer = reinterpret_cast<SharedMemory*>(nativeMap(nullptr, nullptr, fd, true));
    assert(writer != nullptr && writer != MAP_FAILED);
    nativeInit(nullptr, nullptr, reinterpret_cast<jlong>(writer));
    int read_fd = nativeDupAsReadOnly(nullptr, nullptr, fd);
    assert(read_fd >= 0);
    assert(writer->getLatestNetworkTimeUnixEpochMillisAtZeroElapsedRealtimeMillis() == -1);
    assert(writer->systemPic.getMaxNonce() == 128);
    assert(writer->systemPic.getMaxByte() == 8192);
    assert(writer->systemPic.getNonce(0) == 0);
    assert(!writer->systemPic.setNonce(-1, 100));
    assert(!writer->systemPic.setNonce(128, 100));
    writer->setLatestNetworkTimeUnixEpochMillisAtZeroElapsedRealtimeMillis(1234567);
    assert(writer->systemPic.setNonce(127, 998877));
    const signed char data[] = {1, 2, 3, 4};
    writer->systemPic.setByteBlock(42, data, sizeof(data));
    auto* reader = reinterpret_cast<const SharedMemory*>(nativeMap(nullptr, nullptr, read_fd, false));
    assert(reader != nullptr && reader != MAP_FAILED && reader != writer);
    pid_t child = fork();
    assert(child >= 0);
    if (child == 0) {
        assert(munmap(writer, sizeof(SharedMemory)) == 0);
        darwin_art::memory::CloseApplicationDescriptor(fd);
        assert(darwin_art::memory::MapApplicationDescriptor(
                read_fd, sizeof(SharedMemory), true) == MAP_FAILED);
        assert(reader->getLatestNetworkTimeUnixEpochMillisAtZeroElapsedRealtimeMillis() == 1234567);
        assert(reader->systemPic.getNonce(127) == 998877);
        signed char bytes[4]{};
        assert(reader->systemPic.getByteBlock(bytes, sizeof(bytes)) == 42);
        assert(memcmp(bytes, data, sizeof(data)) == 0);
        assert(munmap(const_cast<SharedMemory*>(reader), sizeof(SharedMemory)) == 0);
        darwin_art::memory::CloseApplicationDescriptor(read_fd);
        _exit(0);
    }
    int status = 0;
    assert(waitpid(child, &status, 0) == child);
    assert(WIFEXITED(status) && WEXITSTATUS(status) == 0);
    nativeUnmap(nullptr, nullptr, reinterpret_cast<jlong>(reader));
    nativeUnmap(nullptr, nullptr, reinterpret_cast<jlong>(writer));
    assert(darwin_art::memory::CloseApplicationDescriptor(fd) == 0);
    assert(darwin_art::memory::CloseApplicationDescriptor(read_fd) == 0);
    const int baseline_fds = DescriptorBytes();
    for (int i = 0; i < 100; ++i) {
        int handle = nativeCreate(nullptr, nullptr);
        jlong address = nativeMap(nullptr, nullptr, handle, true);
        assert(address != 0);
        nativeInit(nullptr, nullptr, address);
        int exported = nativeDupAsReadOnly(nullptr, nullptr, handle);
        assert(exported >= 0);
        assert(darwin_art::memory::CloseApplicationDescriptor(handle) == 0);
        nativeUnmap(nullptr, nullptr, address);
        assert(darwin_art::memory::CloseApplicationDescriptor(exported) == 0);
        fail_publication = true;
        assert(darwin_art::memory::CreateApplicationDescriptor(4096) == -1);
        assert(errno == EMFILE);
        fail_publication = false;
        int early = darwin_art::memory::CreateApplicationDescriptor(4096);
        assert(early >= 0);
        assert(darwin_art::memory::CloseApplicationDescriptor(early) == 0);
        int invalid = darwin_art::memory::CreateApplicationMemory(4096);
        assert(invalid >= 0);
        assert(darwin_art::memory::MapApplicationMemory(invalid, 8192, true) == MAP_FAILED);
        assert(close(invalid) == 0);
    }
    assert(DescriptorBytes() == baseline_fds);
    puts("AOSP shared-memory JNI/layout/time/nonce and FD cleanup PASS (isolated FFI table; real Binder not tested)");
}
