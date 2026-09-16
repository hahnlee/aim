package dev.darwinart.runtime.wm;

import android.graphics.Rect;
import android.view.InputChannel;

/** Publishes WMS-owned input-window state through the window's InputChannel. */
final class WindowInputPublisher {
    private WindowInputPublisher() {}

    static void publish(InputChannel serverChannel, Rect frame) {
        if (serverChannel == null) return;
        if (frame == null || frame.isEmpty()) {
            nativePublish(serverChannel, 0, 0, 0, 0, false);
            return;
        }
        nativePublish(serverChannel, frame.left, frame.top, frame.right, frame.bottom, true);
    }

    static void remove(InputChannel serverChannel) {
        if (serverChannel != null) nativePublish(serverChannel, 0, 0, 0, 0, false);
    }

    private static native void nativePublish(InputChannel serverChannel,
            int left, int top, int right, int bottom, boolean visible);
}
