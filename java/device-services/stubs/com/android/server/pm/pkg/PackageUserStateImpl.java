// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm.pkg;
public class PackageUserStateImpl extends com.android.server.utils.WatchableImpl {
    public PackageUserStateImpl(com.android.server.utils.Watchable watchable) { throw new RuntimeException("stub"); }
    public com.android.server.utils.WatchedArrayMap<android.content.pm.UserPackage, SuspendParams> getSuspendParams() { throw new RuntimeException("stub"); }
    public PackageUserStateImpl setSuspendParams(android.util.ArrayMap<android.content.pm.UserPackage, SuspendParams> value) { throw new RuntimeException("stub"); }
    public PackageUserStateImpl putSuspendParams(android.content.pm.UserPackage owner, SuspendParams params) { throw new RuntimeException("stub"); }
    public PackageUserStateImpl removeSuspension(android.content.pm.UserPackage owner) { throw new RuntimeException("stub"); }
    public boolean isSuspended() { throw new RuntimeException("stub"); }
    public boolean isQuarantined() { throw new RuntimeException("stub"); }
    public boolean setOverlayPaths(android.content.pm.overlay.OverlayPaths value) { throw new RuntimeException("stub"); }
    public boolean setSharedLibraryOverlayPaths(String library, android.content.pm.overlay.OverlayPaths value) { throw new RuntimeException("stub"); }
    public android.content.pm.overlay.OverlayPaths getOverlayPaths() { throw new RuntimeException("stub"); }
    public android.content.pm.overlay.OverlayPaths getAllOverlayPaths() { throw new RuntimeException("stub"); }
    public java.util.Map<String, android.content.pm.overlay.OverlayPaths> getSharedLibraryOverlayPaths() { throw new RuntimeException("stub"); }
    public boolean overrideLabelAndIcon(android.content.ComponentName component, String label, Integer icon) { throw new RuntimeException("stub"); }
    public android.util.Pair<String, Integer> getOverrideLabelIconForComponent(android.content.ComponentName component) { throw new RuntimeException("stub"); }
    public PackageUserStateImpl setEnabledComponents(android.util.ArraySet<String> value) { throw new RuntimeException("stub"); }
    public PackageUserStateImpl setDisabledComponents(android.util.ArraySet<String> value) { throw new RuntimeException("stub"); }
    public com.android.server.utils.WatchedArraySet<String> getEnabledComponentsNoCopy() { throw new RuntimeException("stub"); }
    public com.android.server.utils.WatchedArraySet<String> getDisabledComponentsNoCopy() { throw new RuntimeException("stub"); }
    public PackageUserStateImpl setArchiveState(ArchiveState value) { throw new RuntimeException("stub"); }
    public ArchiveState getArchiveState() { throw new RuntimeException("stub"); }
}
