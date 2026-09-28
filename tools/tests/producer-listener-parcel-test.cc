#include <gui/IProducerListener.h>
#include <binder/Parcel.h>
#include <binder/RpcSession.h>
#include <cassert>
#include <cstdio>

class Listener final : public android::BnProducerListener {
public:
    void onBufferReleased() override { ++released; }
    void onBuffersDiscarded(const std::vector<int32_t>& value) override { slots = value; }
    int released = 0;
    std::vector<int32_t> slots;
};

int main() {
    using namespace android;
    auto listener = sp<Listener>::make();
    auto session = RpcSession::make();
    auto transact = [&](uint32_t offset, auto write, auto check) {
        Parcel data, reply;
        data.markForRpc(session);
        reply.markForRpc(session);
        assert(data.writeInterfaceToken(listener->getInterfaceDescriptor()) == OK);
        write(data);
        const auto result = listener->transact(IBinder::FIRST_CALL_TRANSACTION + offset, data, &reply);
        check(result, reply);
    };
    transact(0, [](Parcel&) {}, [](status_t status, Parcel&) { assert(status == OK); });
    assert(listener->released == 1);
    transact(1, [](Parcel&) {}, [](status_t status, Parcel& reply) {
        assert(status == OK);
        bool notify = false;
        assert(reply.readBool(&notify) == OK && notify);
    });
    transact(2, [](Parcel& data) { assert(data.writeInt32Vector({2, 4, 7}) == OK); },
             [](status_t status, Parcel&) { assert(status == OK); });
    assert((listener->slots == std::vector<int32_t>{2, 4, 7}));
    transact(2, [](Parcel& data) { assert(data.writeInt32(2) == OK); },
             [](status_t status, Parcel&) { assert(status != OK); });
    assert((listener->slots == std::vector<int32_t>{2, 4, 7}));
    Parcel wrong, reply;
    wrong.markForRpc(session);
    reply.markForRpc(session);
    assert(wrong.writeInterfaceToken(String16("wrong.interface")) == OK);
    assert(listener->transact(IBinder::FIRST_CALL_TRANSACTION, wrong, &reply) != OK);
    assert(listener->released == 1);
    std::puts("original BnProducerListener Parcel release/notify/discard, malformed vector and wrong token rejection PASS");
}
