package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.view.InputChannel;
import android.view.View;
import java.util.HashMap;

/** Deterministic asynchronous lease/JNI-port fixture for the real WMS path. */
public final class WindowInputPublisher {
    public static boolean failRemove;
    public static int publishCount;
    public static int removeCount;
    private static final class Lease {
        final InputChannel channel;
        int acceptedPrefix;
        Lease(InputChannel value) { channel = value; }
    }
    private static final HashMap<Long, Lease> leases = new HashMap<>();
    private static long nextLease;

    public static void reset() {
        failRemove = false;
        publishCount = 0;
        removeCount = 0;
        leases.clear();
        nextLease = 0;
    }

    public static boolean inputVisible(Rect frame, int visibility) {
        return frame != null && !frame.isEmpty() && visibility == View.VISIBLE;
    }

    public static void publish(InputChannel channel, Rect frame, int visibility) {
        ++publishCount;
    }

    public static void remove(InputChannel channel) {
        ++removeCount;
        if (failRemove) throw new RuntimeException("injected publisher removal failure");
    }

    public static WindowFocusPublicationDelivery.Result decodeStatus(int status) {
        switch (status) {
            case 0: return WindowFocusPublicationDelivery.Result.ACCEPTED;
            case 1: return WindowFocusPublicationDelivery.Result.BACKPRESSURED;
            case 2: return WindowFocusPublicationDelivery.Result.TERMINAL;
            default: throw new IllegalStateException("unknown fixture status");
        }
    }

    static synchronized long nativeAcquireLease(InputChannel channel) {
        if (channel == null) return 0;
        long token = ++nextLease;
        leases.put(token, new Lease(channel));
        return token;
    }

    static synchronized boolean nativeReleaseLease(long token) {
        return leases.remove(token) != null;
    }

    static synchronized int nativeFlushLease(long token) {
        Lease lease = require(token);
        if (lease.acceptedPrefix == 1) lease.acceptedPrefix = 2;
        return 0;
    }

    static synchronized int nativeQueryAcceptedLeaseTx(long token) {
        return require(token).acceptedPrefix;
    }

    static synchronized boolean nativeTerminateLeaseAndQuiesce(long token) {
        require(token);
        return true;
    }

    static synchronized int nativePublishLease(long token, int left, int top,
            int right, int bottom, boolean visible) {
        require(token).acceptedPrefix = 1;
        return 0;
    }

    static synchronized int nativePublishFocusLease(long token, long epoch, boolean focused) {
        require(token).acceptedPrefix = 1;
        return 0;
    }

    private static Lease require(long token) {
        Lease lease = leases.get(token);
        if (lease == null) throw new IllegalStateException("unknown fixture lease");
        return lease;
    }
}
