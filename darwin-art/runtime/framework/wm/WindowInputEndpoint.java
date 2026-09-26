package dev.darwinart.runtime.wm;

import android.view.InputChannel;

/** Original native channel lease; resource operations belong to the WMS driver. */
final class WindowInputEndpoint {
    private final InputChannel original;
    private final long lease;
    boolean retired;

    WindowInputEndpoint(InputChannel original) {
        if (original == null) throw new IllegalArgumentException("missing channel");
        this.original = original;
        lease = WindowInputPublisher.nativeAcquireLease(original);
        if (lease == 0) throw new IllegalStateException("cannot lease original input channel");
    }

    static android.os.IBinder tokenOf(InputChannel channel) {
        if (channel == null) throw new IllegalArgumentException("missing channel");
        android.os.IBinder token = channel.getToken();
        if (token == null) throw new IllegalArgumentException("missing original channel token");
        return token;
    }

    WindowFocusPublicationDelivery.Result publish(WindowFocusRegistry.Publication record) {
        if (record.channelIncarnation != this)
            throw new IllegalArgumentException("foreign input endpoint publication");
        int status = record.kind == WindowFocusRegistry.PublicationKind.GEOMETRY
                ? WindowInputPublisher.nativePublishLease(lease, record.left, record.top,
                        record.right, record.bottom, record.visible,
                        WindowInputPublisher.inputPolicy(record))
                : WindowInputPublisher.nativePublishFocusLease(lease, record.epoch, record.focused);
        return WindowInputPublisher.decodeStatus(status);
    }

    void flush() { WindowInputPublisher.decodeStatus(WindowInputPublisher.nativeFlushLease(lease)); }
    boolean owns(InputChannel channel) { return original == channel; }
    int acceptedPrefix() { return WindowInputPublisher.nativeQueryAcceptedLeaseTx(lease); }
    boolean terminateAndQuiesce() {
        return WindowInputPublisher.nativeTerminateLeaseAndQuiesce(lease);
    }
    void release() {
        if (!WindowInputPublisher.nativeReleaseLease(lease))
            throw new IllegalStateException("original input lease release failed");
        original.dispose();
    }
}
