#include <hidl/HidlSupport.h>
#include <hidl/Status.h>
#include <cutils/native_handle.h>
#include <cassert>
#include <cstdio>
#include <fcntl.h>
#include <unistd.h>

int main() {
    using namespace android::hardware;
    int pipeFds[2];
    assert(pipe(pipeFds) == 0);
    auto* source = native_handle_create(1, 1);
    assert(source);
    source->data[0] = pipeFds[0];
    source->data[1] = 314;
    int ownedFd;
    {
        hidl_handle borrowed(source);
        hidl_handle copy(borrowed);
        assert(copy.getNativeHandle() != source && copy->data[0] != pipeFds[0]);
        assert(native_handle_close(source) == 0);
        assert(native_handle_delete(source) == 0);
        borrowed = static_cast<const native_handle_t*>(nullptr);
        hidl_handle moved(std::move(copy));
        assert(copy.getNativeHandle() == nullptr && moved->data[1] == 314);
        ownedFd = moved->data[0];
        assert(write(pipeFds[1], "H", 1) == 1);
        char value;
        assert(read(ownedFd, &value, 1) == 1 && value == 'H');
    }
    assert(fcntl(ownedFd, F_GETFD) == -1 && errno == EBADF);
    close(pipeFds[1]);
    hidl_string first("original-hidl");
    hidl_string second(first);
    first = "changed";
    assert(second == "original-hidl");
    auto ok = Status::ok();
    auto error = Status::fromExceptionCode(Status::EX_ILLEGAL_ARGUMENT, "bad input");
    assert(ok.isOk() && !error.isOk());
    assert(error.exceptionCode() == Status::EX_ILLEGAL_ARGUMENT);
    std::puts("original HIDL handle clone/move/destruction, string ownership, status PASS");
}
