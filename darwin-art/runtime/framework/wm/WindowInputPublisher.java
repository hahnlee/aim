package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.view.InputChannel;
import android.view.View;
import android.view.WindowManager;

/** Publishes WMS-owned input-window state through the window's InputChannel. */
final class WindowInputPublisher {
    private WindowInputPublisher() {}

    // Window input policy of the WMS window publication, read by the app
    // process's pointer routing (input_window_state.h InputWindowFlags):
    // policy bits, and the window layer in bits 16-31.
    static final int INPUT_TOUCH_MODAL = 1;
    static final int INPUT_WATCH_OUTSIDE_TOUCH = 1 << 1;
    static final int INPUT_NOT_TOUCHABLE = 1 << 2;
    private static final int INPUT_LAYER_SHIFT = 16;
    private static final long MAX_INPUT_LAYER = 0xffff;

    static boolean inputVisible(Rect frame, int viewVisibility) {
        return frame != null && !frame.isEmpty() && viewVisibility == View.VISIBLE;
    }

    /**
     * InputDispatcher's view of a window's LayoutParams flags: touch modal
     * unless not focusable or not touch modal (WindowInfo.isTouchModal), and
     * whether it watches outside touches or takes no touches at all.
     */
    static int inputPolicy(WindowFocusRegistry.Publication record) {
        long layer = Math.max(0, Math.min(MAX_INPUT_LAYER, record.order));
        return inputFlags(record.flags) | (int) (layer << INPUT_LAYER_SHIFT);
    }

    static int inputFlags(int layoutFlags) {
        int result = 0;
        if ((layoutFlags & (WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
                | WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL)) == 0) {
            result |= INPUT_TOUCH_MODAL;
        }
        if ((layoutFlags & WindowManager.LayoutParams.FLAG_WATCH_OUTSIDE_TOUCH) != 0) {
            result |= INPUT_WATCH_OUTSIDE_TOUCH;
        }
        if ((layoutFlags & WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE) != 0) {
            result |= INPUT_NOT_TOUCHABLE;
        }
        return result;
    }

    static WindowFocusPublicationDelivery.Result publish(
            InputChannel serverChannel, Rect frame, int viewVisibility) {
        if (serverChannel == null) return WindowFocusPublicationDelivery.Result.TERMINAL;
        if (frame == null || frame.isEmpty()) {
            return decodeStatus(nativePublish(serverChannel, 0, 0, 0, 0, false, 0));
        }
        return decodeStatus(nativePublish(serverChannel, frame.left, frame.top,
                frame.right, frame.bottom, inputVisible(frame, viewVisibility), 0));
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
                    record.right, record.bottom, record.visible, inputPolicy(record)));
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
            int left, int top, int right, int bottom, boolean visible, int inputFlags);
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
            int left, int top, int right, int bottom, boolean visible, int inputFlags);
    static native int nativePublishFocusLease(long token, long epoch, boolean focused);
}
