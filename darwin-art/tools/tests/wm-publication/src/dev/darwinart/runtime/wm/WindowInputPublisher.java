package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.view.InputChannel;
import android.view.View;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.IdentityHashMap;
import java.util.List;

/**
 * Explicit fixture replacement for the production JNI port.  It records the
 * exact leased InputChannel and models accepted TX, backpressure, terminal
 * quarantine, and delayed flush without native code.
 */
final class WindowInputPublisher {
    static final class Event {
        final InputChannel channel;
        final WindowFocusRegistry.PublicationKind kind;
        final int left;
        final int top;
        final int right;
        final int bottom;
        final boolean visible;
        final long epoch;
        final boolean focused;
        final Object rootToken;
        final Object rootIncarnation;

        Event(InputChannel channel, WindowFocusRegistry.PublicationKind kind,
                int left, int top, int right, int bottom, boolean visible,
                long epoch, boolean focused, Object rootToken, Object rootIncarnation) {
            this.channel = channel;
            this.kind = kind;
            this.left = left;
            this.top = top;
            this.right = right;
            this.bottom = bottom;
            this.visible = visible;
            this.epoch = epoch;
            this.focused = focused;
            this.rootToken = rootToken;
            this.rootIncarnation = rootIncarnation;
        }
    }

    private static final class ChannelState {
        final InputChannel channel;
        final ArrayDeque<Integer> publishResults = new ArrayDeque<>();
        final ArrayDeque<Boolean> terminationResults = new ArrayDeque<>();
        boolean holdFlush;
        boolean immediateAck;
        Runnable flushCallback;
        int flushes;
        int acceptedPrefix;
        int releases;

        ChannelState(InputChannel value) { channel = value; }
    }

    private static final class Lease {
        final long token;
        final ChannelState state;

        Lease(long value, ChannelState channel) {
            token = value;
            state = channel;
        }
    }

    private static final IdentityHashMap<InputChannel, ChannelState> channels =
            new IdentityHashMap<>();
    private static final IdentityHashMap<Long, Lease> leases = new IdentityHashMap<>();
    private static final List<Event> events = new ArrayList<>();
    private static long nextLease;

    private WindowInputPublisher() {}

    static synchronized boolean inputVisible(Rect frame, int viewVisibility) {
        return frame != null && !frame.isEmpty() && viewVisibility == View.VISIBLE;
    }

    static synchronized WindowFocusPublicationDelivery.Result decodeStatus(int status) {
        switch (status) {
            case 0: return WindowFocusPublicationDelivery.Result.ACCEPTED;
            case 1: return WindowFocusPublicationDelivery.Result.BACKPRESSURED;
            case 2: return WindowFocusPublicationDelivery.Result.TERMINAL;
            default: throw new IllegalStateException("unknown fixture status: " + status);
        }
    }

    static synchronized long nativeAcquireLease(InputChannel channel) {
        if (channel == null || channel.disposed) return 0;
        ChannelState state = channels.get(channel);
        if (state == null) {
            state = new ChannelState(channel);
            channels.put(channel, state);
        }
        long token = ++nextLease;
        leases.put(token, new Lease(token, state));
        return token;
    }

    static synchronized boolean nativeReleaseLease(long token) {
        Lease lease = leases.remove(token);
        if (lease == null) return false;
        ++lease.state.releases;
        return true;
    }

    static synchronized int nativeFlushLease(long token) {
        Lease lease = requireLease(token);
        Runnable callback = lease.state.flushCallback;
        lease.state.flushCallback = null;
        ++lease.state.flushes;
        if (callback != null) callback.run();
        if (!lease.state.holdFlush && lease.state.acceptedPrefix == 1)
            lease.state.acceptedPrefix = 2;
        return 0;
    }

    static synchronized int nativeQueryAcceptedLeaseTx(long token) {
        return requireLease(token).state.acceptedPrefix;
    }

    static synchronized boolean nativeTerminateLeaseAndQuiesce(long token) {
        ChannelState state = requireLease(token).state;
        if (state.terminationResults.isEmpty()) return true;
        return state.terminationResults.removeFirst();
    }

    // Fixture: the window input policy is not modeled.
    static int inputPolicy(WindowFocusRegistry.Publication record) {
        return 0;
    }

    static synchronized int nativePublishLease(long token, int left, int top,
            int right, int bottom, boolean visible, int inputFlags) {
        Lease lease = requireLease(token);
        int result = nextPublishResult(lease.state);
        if (result == 0) {
            lease.state.acceptedPrefix = lease.state.immediateAck ? 2 : 1;
            events.add(new Event(lease.state.channel,
                    WindowFocusRegistry.PublicationKind.GEOMETRY,
                    left, top, right, bottom, visible, 0, false,
                    null, null));
        }
        return result;
    }

    static synchronized int nativePublishFocusLease(long token, long epoch,
            boolean focused) {
        Lease lease = requireLease(token);
        int result = nextPublishResult(lease.state);
        if (result == 0) {
            lease.state.acceptedPrefix = lease.state.immediateAck ? 2 : 1;
            events.add(new Event(lease.state.channel,
                    WindowFocusRegistry.PublicationKind.FOCUS,
                    0, 0, 0, 0, true, epoch, focused,
                    null, null));
        }
        return result;
    }

    private static int nextPublishResult(ChannelState state) {
        return state.publishResults.isEmpty() ? 0 : state.publishResults.removeFirst();
    }

    private static ChannelState state(InputChannel channel) {
        ChannelState value = channels.get(channel);
        if (value == null) {
            value = new ChannelState(channel);
            channels.put(channel, value);
        }
        return value;
    }

    private static Lease requireLease(long token) {
        Lease lease = leases.get(token);
        if (lease == null) throw new IllegalStateException("unknown fixture lease");
        return lease;
    }

    static synchronized void reset() {
        channels.clear();
        leases.clear();
        events.clear();
        nextLease = 0;
    }

    static synchronized void scriptPublish(InputChannel channel, int... statuses) {
        ChannelState state = state(channel);
        for (int status : statuses) state.publishResults.addLast(status);
    }

    static synchronized void scriptTermination(InputChannel channel, boolean... outcomes) {
        ChannelState state = state(channel);
        for (boolean outcome : outcomes) state.terminationResults.addLast(outcome);
    }

    static synchronized void holdFlush(InputChannel channel, boolean hold) {
        state(channel).holdFlush = hold;
    }

    static synchronized void immediateAck(InputChannel channel, boolean immediate) {
        state(channel).immediateAck = immediate;
    }

    static synchronized void onNextFlush(InputChannel channel, Runnable callback) {
        state(channel).flushCallback = callback;
    }

    static synchronized int eventCount(InputChannel channel) {
        int count = 0;
        for (Event event : events) if (event.channel == channel) ++count;
        return count;
    }

    static synchronized int eventCount(InputChannel channel,
            WindowFocusRegistry.PublicationKind kind) {
        int count = 0;
        for (Event event : events)
            if (event.channel == channel && event.kind == kind) ++count;
        return count;
    }

    static synchronized int flushCount(InputChannel channel) {
        ChannelState value = channels.get(channel);
        return value == null ? 0 : value.flushes;
    }

    static synchronized boolean released(InputChannel channel) {
        ChannelState state = channels.get(channel);
        return state != null && state.releases != 0;
    }

    static synchronized List<Event> events() {
        return new ArrayList<>(events);
    }
}
