#include <binder/Binder.h>
#include <binder/Parcel.h>
#include <binder/RpcSession.h>
#include <gui/TransactionState.h>
#include <cassert>
#include <cstdio>

int main() {
    using namespace android;
    const sp<IBinder> first = sp<BBinder>::make();
    const sp<IBinder> second = sp<BBinder>::make();
    TransactionState state, incoming;
    state.mId = 41;
    state.mApplyToken = first;
    state.mFlags = 1;
    incoming.mId = 42;
    incoming.mApplyToken = second;
    incoming.mFlags = 2;
    incoming.mMayContainBuffer = true;
    incoming.enableDebugLogCallPoints();
    ComposerState oldLayer, newLayer;
    oldLayer.state.surface = first;
    oldLayer.state.what = layer_state_t::ePositionChanged;
    oldLayer.state.x = 12;
    newLayer.state.surface = first;
    newLayer.state.what = layer_state_t::eAlphaChanged;
    newLayer.state.color.a = 0.25f;
    state.mComposerStates.push_back(oldLayer);
    incoming.mComposerStates.push_back(newLayer);
    state.getDisplayState(first);
    incoming.getDisplayState(second);
    state.merge(std::move(incoming), [](layer_state_t&) { assert(false); });
    assert(state.mId == 41 && state.mApplyToken == first);
    assert(state.mFlags == 3 && state.mMayContainBuffer && state.mLogCallPoints);
    assert(state.mComposerStates.size() == 1 && state.mDisplayStates.size() == 2);
    assert(state.mComposerStates[0].state.x == 12);
    assert(state.mComposerStates[0].state.color.a == 0.25f);
    assert(incoming.mComposerStates.empty() && !incoming.mApplyToken && !incoming.mFlags);
    assert(state.getMergedTransactionIds() == std::vector<uint64_t>{42});
    for (uint64_t id = 100; id < 115; ++id) {
        TransactionState next;
        next.mId = id;
        state.merge(std::move(next), [](layer_state_t&) { assert(false); });
    }
    const auto history = state.getMergedTransactionIds();
    assert(history.size() == 10 && history.front() == 114 && history.back() == 105);
    Parcel wire;
    auto session = RpcSession::make();
    assert(session);
    wire.markForRpc(session);
    assert(state.writeToParcel(&wire) == NO_ERROR);
    wire.setDataPosition(0);
    TransactionState decoded;
    assert(decoded.readFromParcel(&wire) == NO_ERROR);
    assert(decoded.mId == 41 && decoded.mFlags == 3 && decoded.mApplyToken == first);
    assert(decoded.mComposerStates.size() == 1 && decoded.mDisplayStates.size() == 2);
    assert(decoded.mComposerStates[0].state.surface == first);
    assert(decoded.mComposerStates[0].state.color.a == 0.25f);
    assert(decoded.getMergedTransactionIds() == history);
    decoded.clear();
    assert(decoded.mComposerStates.empty() && decoded.mDisplayStates.empty());
    assert(!decoded.mApplyToken && !decoded.mFlags && decoded.getMergedTransactionIds().empty());
    std::puts("libgui TransactionState: real Binder identity, field merge, bounded history, Parcel roundtrip and clear PASS");
}
