// Suspension owner resolution ported from pinned AOSP Settings.java.
// Copyright (C) The Android Open Source Project, Apache License 2.0.
package dev.aim.server;

import android.content.ComponentName;
import android.content.pm.SuspendDialogInfo;
import android.content.pm.UserPackage;
import android.content.pm.overlay.OverlayPaths;
import android.util.ArraySet;
import android.util.Pair;
import com.android.server.pm.pkg.ArchiveState;
import com.android.server.pm.pkg.PackageUserStateInternal;
import com.android.server.pm.pkg.SuspendParams;
import com.android.server.utils.WatchedArrayMap;
import com.android.server.utils.WatchedArraySet;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/** One captured user's interface, with detached original mutable return objects. */
public final class PackageUserStateReplica implements PackageUserStateInternal {
    private final PackageUserStateData data;
    private final Map<UserPackage, PackageUserStateData.Suspension> suspensions;

    public PackageUserStateReplica(PackageUserStateData data, boolean crossUserSuspensions) {
        this.data = Objects.requireNonNull(data);
        var owners = data.getSuspensions();
        if (owners == null) suspensions = null;
        else {
            var resolved = new LinkedHashMap<UserPackage, PackageUserStateData.Suspension>();
            for (var owner : owners) {
                // Pinned Settings.readSuspensionParamsLPr owner resolution.
                int user = owner.resolvedUser ? owner.storedUser : data.getUserId();
                if (!owner.resolvedUser && !owner.currentUser && crossUserSuspensions) {
                    if (owner.storedUser != null && owner.storedUser != -10000) user = owner.storedUser;
                    else if (owner.packageName.equals("android") || owner.packageName.equals("root")
                            || owner.packageName.equals("com.android.shell")) user = 0;
                }
                resolved.put(UserPackage.of(user, owner.packageName), owner);
                if (owner.params != null) dialog(owner.params.dialog); // Validate before publishing.
            }
            var entries = new ArrayList<>(resolved.entrySet());
            entries.sort(java.util.Comparator.comparingInt(entry -> entry.getKey().hashCode()));
            var ordered = new LinkedHashMap<UserPackage, PackageUserStateData.Suspension>();
            for (var entry : entries) ordered.put(entry.getKey(), entry.getValue());
            suspensions = Collections.unmodifiableMap(ordered);
        }
        getArchiveState();
    }
    @Override public long getCeDataInode() { return data.ceDataInode; }
    @Override public long getDeDataInode() { return data.deDataInode; }
    @Override public int getDistractionFlags() { return data.distractionFlags; }
    @Override public int getEnabledState() { return data.enabled; }
    @Override public long getFirstInstallTimeMillis() { return data.firstInstallTime; }
    @Override public String getHarmfulAppWarning() { return data.harmfulWarning; }
    @Override public int getInstallReason() { return data.installReason; }
    @Override public String getLastDisableAppCaller() { return data.lastDisableCaller; }
    @Override public int getMinAspectRatio() { return data.minAspectRatio; }
    @Override public String getSplashScreenTheme() { return data.splashTheme; }
    @Override public int getUninstallReason() { return data.uninstallReason; }
    @Override public boolean isHidden() { return data.hidden; }
    @Override public boolean isInstalled() { return data.installed; }
    @Override public boolean isInstantApp() { return data.instantApp; }
    @Override public boolean isNotLaunched() { return data.notLaunched; }
    @Override public boolean isStopped() { return data.stopped; }
    @Override public boolean isVirtualPreload() { return data.virtualPreload; }
    @Override public boolean dataExists() { return data.ceDataInode > 0 || data.deDataInode > 0; }
    @Override public ArraySet<String> getEnabledComponents() { return components(data.getEnabledComponents()); }
    @Override public ArraySet<String> getDisabledComponents() { return components(data.getDisabledComponents()); }
    private static ArraySet<String> components(String[] values) { return values == null ? new ArraySet<>() : new ArraySet<>(values); }
    private static WatchedArraySet<String> watched(String[] values) {
        if (values == null) return null;
        var set = new WatchedArraySet<String>();
        for (String value : values) set.add(value);
        return set.snapshot();
    }
    @Override public WatchedArraySet<String> getEnabledComponentsNoCopy() { return watched(data.getEnabledComponents()); }
    @Override public WatchedArraySet<String> getDisabledComponentsNoCopy() { return watched(data.getDisabledComponents()); }
    @Override public boolean isComponentEnabled(String component) { return getEnabledComponents().contains(component); }
    @Override public boolean isComponentDisabled(String component) { return getDisabledComponents().contains(component); }
    @Override public boolean isSuspended() { return suspensions != null && !suspensions.isEmpty(); }
    @Override public boolean isQuarantined() {
        if (suspensions != null) for (var owner : suspensions.values()) if (Objects.requireNonNull(owner.params).quarantined) return true;
        return false;
    }
    @Override public WatchedArrayMap<UserPackage, SuspendParams> getSuspendParams() {
        if (suspensions == null) return null;
        var map = new WatchedArrayMap<UserPackage, SuspendParams>();
        for (var entry : suspensions.entrySet()) {
            var params = entry.getValue().params;
            map.put(entry.getKey(), params == null ? null : new SuspendParams(dialog(params.dialog), params.getAppExtras(),
                    params.getLauncherExtras(), params.quarantined));
        }
        return map.snapshot();
    }
    private static SuspendDialogInfo dialog(PackageUserStateData.Dialog value) {
        if (value == null) return null;
        var builder = new SuspendDialogInfo.Builder();
        if (value.icon != 0) builder.setIcon(value.icon);
        if (value.titleResource != 0) builder.setTitle(value.titleResource);
        else if (value.title != null) builder.setTitle(value.title);
        if (value.messageResource != 0) builder.setMessage(value.messageResource);
        else if (value.message != null) builder.setMessage(value.message);
        if (value.buttonResource != 0) builder.setNeutralButtonText(value.buttonResource);
        else if (value.button != null) builder.setNeutralButtonText(value.button);
        return builder.setNeutralButtonAction(value.buttonAction).build();
    }
    private static OverlayPaths paths(PackageUserStateData.Paths value) {
        if (value == null) return null;
        var paths = new OverlayPaths.Builder().build();
        paths.getResourceDirs().addAll(value.resourceDirs);
        paths.getOverlayPaths().addAll(value.overlayPaths);
        return paths;
    }
    @Override public OverlayPaths getOverlayPaths() { return paths(data.overlayPaths); }
    @Override public Map<String, OverlayPaths> getSharedLibraryOverlayPaths() {
        var result = new LinkedHashMap<String, OverlayPaths>();
        if (data.getLibraryOverlays() != null) for (var library : data.getLibraryOverlays()) result.put(library.library, paths(library.paths));
        return Collections.unmodifiableMap(result);
    }
    @Override public OverlayPaths getAllOverlayPaths() {
        if (data.overlayPaths == null && data.getLibraryOverlays() == null) return null;
        var builder = data.overlayPaths == null ? new OverlayPaths.Builder() : new OverlayPaths.Builder(getOverlayPaths());
        for (var paths : getSharedLibraryOverlayPaths().values()) builder.addAll(paths);
        return builder.build();
    }
    @Override public Pair<String, Integer> getOverrideLabelIconForComponent(ComponentName component) {
        Objects.requireNonNull(component);
        if (data.getLabelIcons() != null) for (var value : data.getLabelIcons()) {
            if (value.packageName.equals(component.getPackageName()) && value.className.equals(component.getClassName())) return new Pair<>(value.label, value.icon);
        }
        return null;
    }
    @Override public ArchiveState getArchiveState() {
        var archive = data.archive;
        if (archive == null) return null;
        var activities = new ArrayList<ArchiveState.ArchiveActivityInfo>();
        for (var activity : archive.activities) activities.add(new ArchiveState.ArchiveActivityInfo(activity.title,
            Objects.requireNonNull(ComponentName.unflattenFromString(activity.component)), Path.of(activity.icon),
            activity.monochromeIcon == null ? null : Path.of(activity.monochromeIcon)));
        return new ArchiveState(List.copyOf(activities), archive.installerTitle, archive.time);
    }
}
