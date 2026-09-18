package dev.darwinart.runtime.wm;

import android.os.IBinder;

/** Narrow seam between root authentication/lifetime and WMS publication. */
interface DesktopRootWindowBindingOwner {
    void bind(DesktopRootRegistry.Registration root, IBinder originalChannelToken, int displayId);
    void unbind(DesktopRootRegistry.Registration root);
    void factChanged(DesktopRootRegistry.Registration root, DesktopRootRegistry.Fact fact);
    default void attachFocusDecisions(DesktopRootRegistry.Registration root, IBinder callback) {
        throw new UnsupportedOperationException("desktop focus decisions unavailable");
    }
}
