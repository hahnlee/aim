package dev.aim.server;

import android.content.pm.ActivityInfo;
import android.content.pm.ApplicationInfo;
import android.content.pm.overlay.OverlayPaths;
import java.util.Objects;

/** Original boot-selected application/resolver metadata, shared with the live resolver owner. */
public final class NativeSystemOverlayOwner {
    private final ApplicationInfo platform;
    private final ActivityInfo resolver;
    private final boolean replaced;
    public NativeSystemOverlayOwner(ApplicationInfo platform, ActivityInfo resolver, boolean replaced) {
        this.platform = platform;
        this.resolver = Objects.requireNonNull(resolver);
        this.replaced = replaced;
    }
    public synchronized void update(String target, OverlayPaths paths) {
        if (!replaced && target.equals("android")) {
            if (platform != null) {
                platform.overlayPaths = paths == null ? null : paths.getOverlayPaths().toArray(new String[0]);
                platform.resourceDirs = paths == null ? null : paths.getResourceDirs().toArray(new String[0]);
            } else android.util.Slog.i("PackageManager", "Skipped the AndroidApplication overlay paths update - no app yet");
        } else if (replaced && target.equals(resolver.applicationInfo.packageName)) {
            resolver.applicationInfo.overlayPaths = paths == null ? null : paths.getOverlayPaths().toArray(new String[0]);
            resolver.applicationInfo.resourceDirs = paths == null ? null : paths.getResourceDirs().toArray(new String[0]);
        }
    }
}
