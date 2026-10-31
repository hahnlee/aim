package dev.aim.server;
import dev.aim.server.AccessibilityWindowData;
/** Real native window snapshots and rectangle requests. */
oneway interface IAccessibilityWindowCallback {
    void onWindowsChanged(int displayId, int width, int height, IBinder focused,
            in AccessibilityWindowData[] windows);
    void onRectangleRequested(int displayId, int left, int top, int right, int bottom);
}
