package dev.aim.server;

import android.content.Context;
import android.graphics.*;
import android.os.*;
import android.util.*;
import android.view.*;
import android.view.inputmethod.ImeTracker;
import android.window.ScreenCapture;
import android.window.ScreenCapture.ScreenshotHardwareBuffer;
import com.android.internal.policy.KeyInterceptionInfo;
import com.android.server.wm.SensitiveContentPackages.PackageInfo;
import java.util.*;
import com.android.server.LocalServices;
import com.android.server.wm.WindowManagerInternal;

/** Typed local adapter to the native window owner; original accessibility policy stays in services.jar. */
final class AccessibilityWindows extends WindowManagerInternal {
    private final IAccessibilityWindowHost host;
    AccessibilityWindows(IAccessibilityWindowHost host) { this.host = host; }
    static void publish(ISystemProcess system) throws RemoteException {
        LocalServices.addService(WindowManagerInternal.class,
                new AccessibilityWindows(system.getAccessibilityWindowHost()));
    }
    private static UnsupportedOperationException unsupported() {
        return new UnsupportedOperationException("WindowManagerInternal outside accessibility lifecycle (#2529)");
    }
    @Override public void registerWindowFocusChangeListener(WindowFocusChangeListener listener) { throw unsupported(); }
    @Override public void unregisterWindowFocusChangeListener(WindowFocusChangeListener listener) { throw unsupported(); }
    private static final class UiChanges extends IAccessibilityWindowCallback.Stub {
        private final AccessibilityControllerInternal.UiChangesForAccessibilityCallbacks target;
        UiChanges(AccessibilityControllerInternal.UiChangesForAccessibilityCallbacks target) { this.target = target; }
        @Override public void onWindowsChanged(int d, int w, int h, IBinder f, AccessibilityWindowData[] v) { throw unsupported(); }
        @Override public void onRectangleRequested(int d, int l, int t, int r, int b) {
            target.onRectangleOnScreenRequested(d, l, t, r, b);
        }
    }
    @Override public AccessibilityControllerInternal getAccessibilityController() {
        return new AccessibilityControllerInternal() {
            @Override public void startTrace(long flags) { throw unsupported(); }
            @Override public void stopTrace() { throw unsupported(); }
            @Override public void logTrace(String w, long f, String p, byte[] d, int u, StackTraceElement[] s, Set<String> i) { throw unsupported(); }
            @Override public void logTrace(String w, long f, String p, byte[] d, int u, StackTraceElement[] s, long t, int pid, long tid, Set<String> i) { throw unsupported(); }
            @Override public void setUiChangesForAccessibilityCallbacks(UiChangesForAccessibilityCallbacks target) {
                UiChanges ui = target == null ? null : new UiChanges(target);
                call(() -> host.setUiCallback(ui));
            }
        };
    }
    private interface RemoteCall { void run() throws RemoteException; }
    private static void call(RemoteCall action) {
        try { action.run(); } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    private final Map<Integer, IAccessibilityWindowCallback> callbacks = new HashMap<>();
    private static final class NativeWindow extends com.android.server.wm.AccessibilityWindowsPopulator$AccessibilityWindow {
        private final WindowInfo info = WindowInfo.obtain();
        private final Region region;
        private final int flags, privateFlags;
        NativeWindow(AccessibilityWindowData v, int display) {
            info.token = v.token; info.type = v.type; info.displayId = display;
            flags = v.flags; privateFlags = v.privateFlags;
            region = new Region(new Rect(v.left, v.top, v.right, v.bottom));
            info.focused = v.focused; info.layer = v.layer;
        }
        @Override public void getTouchableRegionInScreen(Region out) { out.set(region); }
        @Override public void getTouchableRegionInWindow(Region out) { out.set(region); }
        @Override public int getType() { return info.type; }
        @Override public WindowInfo getWindowInfo() { return info; }
        @Override public boolean shouldMagnify() { return (privateFlags & 0x200000) == 0; }
        @Override public boolean isFocused() { return info.focused; }
        @Override public boolean isTrustedOverlay() { return (privateFlags & 0x20000000) != 0; }
        @Override public boolean isTouchable() { return (flags & 0x10) == 0; }
        @Override public boolean isUntouchableNavigationBar() { return info.type == 2019 && !isTouchable(); }
        @Override public boolean isPIPMenu() { return false; }
        @Override public Rect getSystemBarInsetsFrame() { return null; }
    }
    @Override public synchronized void setWindowsForAccessibilityCallback(int displayId, WindowsForAccessibilityCallback target) {
        IAccessibilityWindowCallback callback = target == null ? null : new IAccessibilityWindowCallback.Stub() {
            @Override public void onWindowsChanged(int d, int width, int height, IBinder focused, AccessibilityWindowData[] values) {
                synchronized (AccessibilityWindows.this) {
                    if (callbacks.get(d) != this) return;
                }
                    List<com.android.server.wm.AccessibilityWindowsPopulator$AccessibilityWindow> windows = new ArrayList<>();
                    for (AccessibilityWindowData value : values) windows.add(new NativeWindow(value, d));
                    target.onAccessibilityWindowsChanged(true, d, focused, new Point(width, height), windows);
            }
            @Override public void onRectangleRequested(int d, int l, int t, int r, int b) { throw unsupported(); }
        };
        IAccessibilityWindowCallback old = callbacks.put(displayId, callback);
        try { host.setCallback(displayId, callback); }
        catch (RemoteException | RuntimeException e) { callbacks.put(displayId, old); throw new IllegalStateException("window callback registration", e); }
    }
    @Override public void requestTraversalFromDisplayManager() { throw unsupported(); }
    @Override public void onDisplayManagerReceivedDeviceState(int deviceState) { throw unsupported(); }
    @Override public boolean setMagnificationCallbacks(int displayId, MagnificationCallbacks callbacks) { throw unsupported(); }
    @Override public void setMagnificationSpec(int displayId, MagnificationSpec spec) { throw unsupported(); }
    @Override public void setFullscreenMagnificationActivated(int displayId, boolean activated) { throw unsupported(); }
    @Override public void getMagnificationRegion(int displayId, Region magnificationRegion) { throw unsupported(); }
    @Override public void setInputFilter(IInputFilter filter) { throw unsupported(); }
    @Override public IBinder getFocusedWindowToken() { throw unsupported(); }
    @Override public IBinder getFocusedWindowTokenFromWindowStates() {
        try { return host.getFocusedWindowTokenFromWindowStates(); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public void moveDisplayToTopIfAllowed(int displayId) { throw unsupported(); }
    @Override public void requestWindowFocus(IBinder windowToken) { throw unsupported(); }
    @Override public boolean isKeyguardLocked() { throw unsupported(); }
    @Override public boolean isKeyguardShowingAndNotOccluded() { throw unsupported(); }
    @Override public boolean isKeyguardSecure(int userId) { throw unsupported(); }
    @Override public void getWindowFrame(IBinder token, Rect outBounds) { throw unsupported(); }
    @Override public Pair<Matrix, MagnificationSpec> getWindowTransformationMatrixAndMagnificationSpec(IBinder token) {
        try {
            Matrix matrix = new Matrix();
            matrix.setValues(host.getWindowTransformationMatrix(token));
            // No magnifier can be activated: setMagnificationCallbacks/Spec reject it above.
            return new Pair<>(matrix, new MagnificationSpec());
        } catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public void showGlobalActions() { throw unsupported(); }
    @Override public void waitForAllWindowsDrawn(Message message, long timeout, int displayId) { throw unsupported(); }
    @Override public void setForcedDisplaySize(int displayId, int width, int height) { throw unsupported(); }
    @Override public void clearForcedDisplaySize(int displayId) { throw unsupported(); }
    @Override public void addWindowToken(android.os.IBinder token, int type, int displayId, Bundle options) {
        if (type != 2032 || options != null) throw unsupported();
        call(() -> host.addOverlayToken(token, displayId));
    }
    @Override public void removeWindowToken(android.os.IBinder token, boolean removeWindows, boolean animateExit, int displayId) {
        if (!removeWindows) throw unsupported();
        call(() -> host.removeOverlayToken(token, displayId));
    }
    @Override public void registerAppTransitionListener(AppTransitionListener listener) { throw unsupported(); }
    @Override public void registerTaskSystemBarsListener(TaskSystemBarsListener listener) { throw unsupported(); }
    @Override public void unregisterTaskSystemBarsListener(TaskSystemBarsListener listener) { throw unsupported(); }
    @Override public void reportPasswordChanged(int userId) { throw unsupported(); }
    @Override public int getInputMethodWindowVisibleHeight(int displayId) { throw unsupported(); }
    @Override public void setDismissImeOnBackKeyPressed(boolean dismissImeOnBackKeyPressed) { throw unsupported(); }
    @Override public void updateInputMethodTargetWindow(IBinder imeTargetWindowToken) { throw unsupported(); }
    @Override public boolean isHardKeyboardAvailable() { throw unsupported(); }
    @Override public void setOnHardKeyboardStatusChangeListener( OnHardKeyboardStatusChangeListener listener) { throw unsupported(); }
    @Override public void setOnImeRequestedChangedListener(OnImeRequestedChangedListener listener) { throw unsupported(); }
    @Override public void computeWindowsForAccessibility(int displayId) { call(() -> host.computeWindows(displayId)); }
    @Override public void setVr2dDisplayId(int vr2dDisplayId) { throw unsupported(); }
    @Override public void registerDragDropControllerCallback(IDragDropCallback callback) { throw unsupported(); }
    @Override public void lockNow() { throw unsupported(); }
    @Override public int getWindowOwnerUserId(IBinder windowToken) {
        try { return host.getWindowOwnerUserId(windowToken); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public void setWallpaperShowWhenLocked(IBinder windowToken, boolean showWhenLocked) { throw unsupported(); }
    @Override public void setWallpaperCropHints(IBinder windowToken, SparseArray<Rect> cropHints) { throw unsupported(); }
    @Override public boolean isUidFocused(int uid) { throw unsupported(); }
    @Override public int hasInputMethodClientFocus(IBinder windowToken, int uid, int pid, int displayId) { throw unsupported(); }
    @Override public boolean isUidAllowedOnDisplay(int displayId, int uid) { throw unsupported(); }
    @Override public int getDisplayIdForWindow(IBinder windowToken) {
        try { return host.getDisplayIdForWindow(windowToken); }
        catch (RemoteException e) { throw e.rethrowFromSystemServer(); }
    }
    @Override public int getTopFocusedDisplayId() { throw unsupported(); }
    @Override public Context getTopFocusedDisplayUiContext() { throw unsupported(); }
    @Override public void setNonDefaultDisplayRotation(int displayId, int rotation, String caller) { throw unsupported(); }
    @Override public void setHomeSupportedOnDisplay( String displayUniqueId, int displayType, boolean supported) { throw unsupported(); }
    @Override public boolean isHomeSupportedOnDisplay(int displayId) { throw unsupported(); }
    @Override public void setIgnoreActivitySizeRestrictionsOnDisplay( String displayUniqueId, int displayType, boolean enabled) { throw unsupported(); }
    @Override public void clearDisplaySettings(String displayUniqueId, int displayType) { throw unsupported(); }
    @Override public int getDisplayImePolicy(int displayId) { throw unsupported(); }
    @Override public void showImePostLayout(IBinder imeTargetWindowToken, ImeTracker.Token statsToken) { throw unsupported(); }
    @Override public void hideIme(IBinder imeTargetWindowToken, int displayId, ImeTracker.Token statsToken) { throw unsupported(); }
    @Override public void addRefreshRateRangeForPackage(String packageName, float minRefreshRate, float maxRefreshRate) { throw unsupported(); }
    @Override public void removeRefreshRateRangeForPackage(String packageName) { throw unsupported(); }
    @Override public boolean isTouchOrFaketouchDevice() { throw unsupported(); }
    @Override public KeyInterceptionInfo getKeyInterceptionInfoFromToken(IBinder inputToken) { throw unsupported(); }
    @Override public void clearSnapshotCache() { throw unsupported(); }
    @Override public void setAccessibilityIdToSurfaceMetadata(IBinder windowToken, int accessibilityWindowId) {
        call(() -> host.setAccessibilityIdToSurfaceMetadata(windowToken, accessibilityWindowId));
    }
    @Override public String getWindowName(IBinder binder) { throw unsupported(); }
    @Override public ImeTargetInfo onToggleImeRequested(boolean show, IBinder focusedToken, IBinder requestToken, int displayId) { throw unsupported(); }
    @Override public IBinder getTargetWindowTokenFromInputToken(IBinder inputToken) { throw unsupported(); }
    @Override public void moveWindowTokenToDisplay(IBinder binder, int displayId) { throw unsupported(); }
    @Override public boolean shouldRestoreImeVisibility(IBinder imeTargetWindowToken) { throw unsupported(); }
    @Override public void addTrustedTaskOverlay(int taskId, SurfaceControlViewHost.SurfacePackage overlay) { throw unsupported(); }
    @Override public void removeTrustedTaskOverlay(int taskId, SurfaceControlViewHost.SurfacePackage overlay) { throw unsupported(); }
    @Override public SurfaceControl getHandwritingSurfaceForDisplay(int displayId) { throw unsupported(); }
    @Override public boolean isPointInsideWindow( IBinder windowToken, int displayId, float displayX, float displayY) { throw unsupported(); }
    @Override public boolean setContentRecordingSession(ContentRecordingSession incomingSession) { throw unsupported(); }
    @Override public SurfaceControl getA11yOverlayLayer(int displayId) { throw unsupported(); }
    @Override public void captureDisplay(int displayId, ScreenCapture.CaptureArgs captureArgs, ScreenCapture.ScreenCaptureListener listener) { throw unsupported(); }
    @Override public boolean hasNavigationBar(int displayId) { throw unsupported(); }
    @Override public void setOrientationRequestPolicy(boolean respected, int[] fromOrientations, int[] toOrientations) { throw unsupported(); }
    @Override public void setBlockScreenCaptureForAppsSessionId(long sessionId) { throw unsupported(); }
    @Override public void addBlockScreenCaptureForApps(ArraySet<PackageInfo> packageInfos) { throw unsupported(); }
    @Override public void removeBlockScreenCaptureForApps( ArraySet<PackageInfo> packageInfos) { throw unsupported(); }
    @Override public void clearBlockedApps() { throw unsupported(); }
    @Override public void registerOnWindowRemovedListener(OnWindowRemovedListener listener) { throw unsupported(); }
    @Override public void unregisterOnWindowRemovedListener(OnWindowRemovedListener listener) { throw unsupported(); }
    @Override public boolean moveFocusToAdjacentEmbeddedActivityIfNeeded() { throw unsupported(); }
    @Override public ScreenshotHardwareBuffer takeAssistScreenshot() { throw unsupported(); }
    @Override public ScreenshotHardwareBuffer takeContextualSearchScreenshot(int uid) { throw unsupported(); }
}
