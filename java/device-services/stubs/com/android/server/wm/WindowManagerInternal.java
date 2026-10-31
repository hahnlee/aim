// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
// ABI: AOSP android-16.0.0_r1 (Apache-2.0).
package com.android.server.wm;
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
public abstract class WindowManagerInternal {
    public WindowManagerInternal() { throw new RuntimeException("stub"); }
    public interface WindowFocusChangeListener { void focusChanged(IBinder token); }
    public interface WindowsForAccessibilityCallback {
        void onAccessibilityWindowsChanged(boolean force, int display, IBinder focused,
                Point size, List<AccessibilityWindowsPopulator$AccessibilityWindow> windows);
    }
    public interface AccessibilityControllerInternal {
        void startTrace(long flags);
        void stopTrace();
        void logTrace(String where, long flags, String params, byte[] dump, int uid, StackTraceElement[] stack, Set<String> ignore);
        void logTrace(String where, long flags, String params, byte[] dump, int uid, StackTraceElement[] stack, long time, int pid, long tid, Set<String> ignore);
        void setUiChangesForAccessibilityCallbacks(UiChangesForAccessibilityCallbacks callbacks);
        interface UiChangesForAccessibilityCallbacks { void onRectangleOnScreenRequested(int display, int left, int top, int right, int bottom); }
    }
    public interface MagnificationCallbacks {}
    public static abstract class AppTransitionListener {}
    public interface TaskSystemBarsListener {}
    public interface OnHardKeyboardStatusChangeListener {}
    public interface OnImeRequestedChangedListener {}
    public interface IDragDropCallback {}
    public interface OnWindowRemovedListener {}
    public static final class ImeTargetInfo { public ImeTargetInfo(String a, String b, String c, String d, String e) { throw new RuntimeException("stub"); } }
    public abstract void registerWindowFocusChangeListener(WindowFocusChangeListener listener);
    public abstract void unregisterWindowFocusChangeListener(WindowFocusChangeListener listener);
    public abstract AccessibilityControllerInternal getAccessibilityController();
    public abstract void requestTraversalFromDisplayManager();
    public abstract void onDisplayManagerReceivedDeviceState(int deviceState);
    public abstract boolean setMagnificationCallbacks(int displayId, MagnificationCallbacks callbacks);
    public abstract void setMagnificationSpec(int displayId, MagnificationSpec spec);
    public abstract void setFullscreenMagnificationActivated(int displayId, boolean activated);
    public abstract void getMagnificationRegion(int displayId, Region magnificationRegion);
    public abstract void setWindowsForAccessibilityCallback(int displayId, WindowsForAccessibilityCallback callback);
    public abstract void setInputFilter(IInputFilter filter);
    public abstract IBinder getFocusedWindowToken();
    public abstract IBinder getFocusedWindowTokenFromWindowStates();
    public abstract void moveDisplayToTopIfAllowed(int displayId);
    public abstract void requestWindowFocus(IBinder windowToken);
    public abstract boolean isKeyguardLocked();
    public abstract boolean isKeyguardShowingAndNotOccluded();
    public abstract boolean isKeyguardSecure(int userId);
    public abstract void getWindowFrame(IBinder token, Rect outBounds);
    public abstract Pair<Matrix, MagnificationSpec> getWindowTransformationMatrixAndMagnificationSpec(IBinder token);
    public abstract void showGlobalActions();
    public abstract void waitForAllWindowsDrawn(Message message, long timeout, int displayId);
    public abstract void setForcedDisplaySize(int displayId, int width, int height);
    public abstract void clearForcedDisplaySize(int displayId);
    public abstract void addWindowToken(android.os.IBinder token, int type, int displayId, Bundle options);
    public abstract void removeWindowToken(android.os.IBinder token, boolean removeWindows, boolean animateExit, int displayId);
    public abstract void registerAppTransitionListener(AppTransitionListener listener);
    public abstract void registerTaskSystemBarsListener(TaskSystemBarsListener listener);
    public abstract void unregisterTaskSystemBarsListener(TaskSystemBarsListener listener);
    public abstract void reportPasswordChanged(int userId);
    public abstract int getInputMethodWindowVisibleHeight(int displayId);
    public abstract void setDismissImeOnBackKeyPressed(boolean dismissImeOnBackKeyPressed);
    public abstract void updateInputMethodTargetWindow(IBinder imeTargetWindowToken);
    public abstract boolean isHardKeyboardAvailable();
    public abstract void setOnHardKeyboardStatusChangeListener( OnHardKeyboardStatusChangeListener listener);
    public abstract void setOnImeRequestedChangedListener(OnImeRequestedChangedListener listener);
    public abstract void computeWindowsForAccessibility(int displayId);
    public abstract void setVr2dDisplayId(int vr2dDisplayId);
    public abstract void registerDragDropControllerCallback(IDragDropCallback callback);
    public abstract void lockNow();
    public abstract int getWindowOwnerUserId(IBinder windowToken);
    public abstract void setWallpaperShowWhenLocked(IBinder windowToken, boolean showWhenLocked);
    public abstract void setWallpaperCropHints(IBinder windowToken, SparseArray<Rect> cropHints);
    public abstract boolean isUidFocused(int uid);
    public abstract int hasInputMethodClientFocus(IBinder windowToken, int uid, int pid, int displayId);
    public abstract boolean isUidAllowedOnDisplay(int displayId, int uid);
    public abstract int getDisplayIdForWindow(IBinder windowToken);
    public abstract int getTopFocusedDisplayId();
    public abstract Context getTopFocusedDisplayUiContext();
    public abstract void setNonDefaultDisplayRotation(int displayId, int rotation, String caller);
    public abstract void setHomeSupportedOnDisplay( String displayUniqueId, int displayType, boolean supported);
    public abstract boolean isHomeSupportedOnDisplay(int displayId);
    public abstract void setIgnoreActivitySizeRestrictionsOnDisplay( String displayUniqueId, int displayType, boolean enabled);
    public abstract void clearDisplaySettings(String displayUniqueId, int displayType);
    public abstract int getDisplayImePolicy(int displayId);
    public abstract void showImePostLayout(IBinder imeTargetWindowToken, ImeTracker.Token statsToken);
    public abstract void hideIme(IBinder imeTargetWindowToken, int displayId, ImeTracker.Token statsToken);
    public abstract void addRefreshRateRangeForPackage(String packageName, float minRefreshRate, float maxRefreshRate);
    public abstract void removeRefreshRateRangeForPackage(String packageName);
    public abstract boolean isTouchOrFaketouchDevice();
    public abstract KeyInterceptionInfo getKeyInterceptionInfoFromToken(IBinder inputToken);
    public abstract void clearSnapshotCache();
    public abstract void setAccessibilityIdToSurfaceMetadata( IBinder windowToken, int accessibilityWindowId);
    public abstract String getWindowName(IBinder binder);
    public abstract ImeTargetInfo onToggleImeRequested(boolean show, IBinder focusedToken, IBinder requestToken, int displayId);
    public abstract IBinder getTargetWindowTokenFromInputToken(IBinder inputToken);
    public abstract void moveWindowTokenToDisplay(IBinder binder, int displayId);
    public abstract boolean shouldRestoreImeVisibility(IBinder imeTargetWindowToken);
    public abstract void addTrustedTaskOverlay(int taskId, SurfaceControlViewHost.SurfacePackage overlay);
    public abstract void removeTrustedTaskOverlay(int taskId, SurfaceControlViewHost.SurfacePackage overlay);
    public abstract SurfaceControl getHandwritingSurfaceForDisplay(int displayId);
    public abstract boolean isPointInsideWindow( IBinder windowToken, int displayId, float displayX, float displayY);
    public abstract boolean setContentRecordingSession(ContentRecordingSession incomingSession);
    public abstract SurfaceControl getA11yOverlayLayer(int displayId);
    public abstract void captureDisplay(int displayId, ScreenCapture.CaptureArgs captureArgs, ScreenCapture.ScreenCaptureListener listener);
    public abstract boolean hasNavigationBar(int displayId);
    public abstract void setOrientationRequestPolicy(boolean respected, int[] fromOrientations, int[] toOrientations);
    public abstract void setBlockScreenCaptureForAppsSessionId(long sessionId);
    public abstract void addBlockScreenCaptureForApps(ArraySet<PackageInfo> packageInfos);
    public abstract void removeBlockScreenCaptureForApps( ArraySet<PackageInfo> packageInfos);
    public abstract void clearBlockedApps();
    public abstract void registerOnWindowRemovedListener(OnWindowRemovedListener listener);
    public abstract void unregisterOnWindowRemovedListener(OnWindowRemovedListener listener);
    public abstract boolean moveFocusToAdjacentEmbeddedActivityIfNeeded();
    public abstract ScreenshotHardwareBuffer takeAssistScreenshot();
    public abstract ScreenshotHardwareBuffer takeContextualSearchScreenshot(int uid);
}
