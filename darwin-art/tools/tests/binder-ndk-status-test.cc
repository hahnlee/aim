#include <android/binder_status.h>
#include <cassert>
#include <cstdio>
#include <cstring>

int main() {
    AStatus* ok = AStatus_newOk();
    assert(AStatus_isOk(ok));
    assert(AStatus_getExceptionCode(ok) == EX_NONE);
    assert(AStatus_getStatus(ok) == STATUS_OK);
    AStatus_delete(ok);
    assert(AStatus_isOk(AStatus_newOk()));

    AStatus* service = AStatus_fromServiceSpecificErrorWithMessage(73, "allocator rejected usage");
    assert(!AStatus_isOk(service));
    assert(AStatus_getExceptionCode(service) == EX_SERVICE_SPECIFIC);
    assert(AStatus_getServiceSpecificError(service) == 73);
    assert(AStatus_getStatus(service) == STATUS_OK);
    assert(std::strcmp(AStatus_getMessage(service), "allocator rejected usage") == 0);
    const char* description = AStatus_getDescription(service);
    AStatus_delete(service);
    // Description is a separately owned copy, not a pointer into deleted Status.
    assert(std::strstr(description, "allocator rejected usage"));
    AStatus_deleteDescription(description);

    AStatus* transport = AStatus_fromStatus(STATUS_DEAD_OBJECT);
    assert(AStatus_getExceptionCode(transport) == EX_TRANSACTION_FAILED);
    assert(AStatus_getStatus(transport) == STATUS_DEAD_OBJECT);
    AStatus_delete(transport);
    AStatus* unknown = AStatus_fromStatus(12345);
    assert(AStatus_getStatus(unknown) == STATUS_UNKNOWN_ERROR);
    AStatus_delete(unknown);
    AStatus* unknown_exception = AStatus_fromExceptionCode(12345);
    assert(AStatus_getExceptionCode(unknown_exception) == EX_TRANSACTION_FAILED);
    AStatus_delete(unknown_exception);

    for (int i = 0; i < 1000; ++i) {
        AStatus* security = AStatus_fromExceptionCodeWithMessage(EX_SECURITY, "denied");
        assert(AStatus_getExceptionCode(security) == EX_SECURITY);
        assert(std::strcmp(AStatus_getMessage(security), "denied") == 0);
        AStatus_delete(security);
    }
    std::puts("original Binder NDK Status: exceptions, service/transport errors, description ownership PASS");
}
