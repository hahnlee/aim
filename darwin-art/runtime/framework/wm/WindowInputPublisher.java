package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.view.InputChannel;
import android.view.View;

/** Publishes WMS-owned input-window state through the window's InputChannel. */
final class WindowInputPublisher {
    private WindowInputPublisher() {}

    static boolean inputVisible(Rect frame, int viewVisibility) {
        return frame != null && !frame.isEmpty() && viewVisibility == View.VISIBLE;
    }

    static WindowFocusPublicationDelivery.Result publish(
            InputChannel serverChannel, Rect frame, int viewVisibility) {
        if (serverChannel == null) return WindowFocusPublicationDelivery.Result.TERMINAL;
        if (frame == null || frame.isEmpty()) {
            return decodeStatus(nativePublish(serverChannel, 0, 0, 0, 0, false));
        }
        return decodeStatus(nativePublish(serverChannel, frame.left, frame.top,
                frame.right, frame.bottom, inputVisible(frame, viewVisibility)));
    }

    static WindowFocusPublicationDelivery.Result remove(InputChannel serverChannel) {
        return publish(serverChannel, null, View.GONE);
    }

    /** Uses the recorded endpoint, never the current window-to-channel map. */
    static WindowFocusPublicationDelivery.Result send(WindowFocusRegistry.Publication record) {
        if (record == null || !(record.channelIncarnation instanceof InputChannel))
            throw new IllegalArgumentException("publication requires its original server channel");
        InputChannel channel = (InputChannel) record.channelIncarnation;
        if (record.kind == WindowFocusRegistry.PublicationKind.GEOMETRY)
            return decodeStatus(nativePublish(channel, record.left, record.top,
                    record.right, record.bottom, record.visible));
        if (record.kind == WindowFocusRegistry.PublicationKind.FOCUS)
            return decodeStatus(nativePublishFocus(channel, record.epoch, record.focused));
        throw new IllegalArgumentException("unknown publication kind");
    }

    static WindowFocusPublicationDelivery.Result decodeStatus(int status) {
        switch (status) {
            case 0: return WindowFocusPublicationDelivery.Result.ACCEPTED;
            case 1: return WindowFocusPublicationDelivery.Result.BACKPRESSURED;
            case 2: return WindowFocusPublicationDelivery.Result.TERMINAL;
            default: throw new IllegalStateException("unknown input publication status: " + status);
        }
    }

    private static native int nativePublish(InputChannel serverChannel,
            int left, int top, int right, int bottom, boolean visible);
    private static native int nativePublishFocus(InputChannel serverChannel,
            long epoch, boolean focused);

    // Preparatory exact-endpoint port: callers must retain this token until
    // publication/TX obligations settle. Release is not delivery or settlement.
    // Session adoption waits for that owner; never replace it with Java dispose.
    static native long nativeAcquireLease(InputChannel serverChannel);
    static native boolean nativeReleaseLease(long token);
    // Actual original-TX terminal/quiescent check; false requires later progress.
    static native boolean nativeTerminateLeaseAndQuiesce(long token);
    // Flushes retained bytes on the exact original transport.
    static native int nativeFlushLease(long token);
    // Queries only the accepted prefix captured from the exact original transport.
    static native int nativeQueryAcceptedLeaseTx(long token);
    static native int nativePublishLease(long token,
            int left, int top, int right, int bottom, boolean visible);
    static native int nativePublishFocusLease(long token, long epoch, boolean focused);
}
