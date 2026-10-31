package dev.aim.server;
import dev.aim.server.IAccessibilityWindowCallback;
/** Native window ownership, system UID only (#2529). */
interface IAccessibilityWindowHost {
    void addOverlayToken(IBinder token, int displayId);
    void removeOverlayToken(IBinder token, int displayId);
    void setCallback(int displayId, IAccessibilityWindowCallback callback);
    void computeWindows(int displayId);
    void setUiCallback(IAccessibilityWindowCallback callback);
    void setAccessibilityServiceUids(in int[] uids);
}
