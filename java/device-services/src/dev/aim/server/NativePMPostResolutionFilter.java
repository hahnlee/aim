package dev.aim.server;

import android.content.ComponentName;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.AuxiliaryResolveInfo;
import android.content.pm.ResolveInfo;
import com.android.server.pm.NativeComputer;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/** Original ComputerEngine post-resolution policy over the same retained native owners. */
public final class NativePMPostResolutionFilter implements NativeComputer.QueryOwner {
    private final PackageSnapshots.ComputerSnapshot packages;
    private final Map<Integer, Boolean> webDisabled;
    public NativePMPostResolutionFilter(PackageSnapshots.ComputerSnapshot packages, long version,
            Map<Integer, Boolean> capturedWebDisabled) {
        this.packages = Objects.requireNonNull(packages);
        if (version != packages.getVersion()) throw new IllegalArgumentException("web instant policy capture differs");
        this.webDisabled = Map.copyOf(capturedWebDisabled);
    }
    @Override public List<ResolveInfo> applyPostResolutionFilter(List<ResolveInfo> infos,
            String ephemeral, boolean dynamic, int filterUid, boolean forStart, int user, Intent intent) {
        packages.getVersion();
        boolean blockInstant = intent.isWebIntent() && webDisabled.getOrDefault(user, false);
        for (int i = infos.size() - 1; i >= 0; i--) {
            ResolveInfo info = infos.get(i);
            if (info.isInstantAppAvailable && blockInstant) { infos.remove(i); continue; }
            var activity = info.activityInfo;
            if (dynamic && activity != null && activity.splitName != null
                    && !contains(activity.applicationInfo.splitNames, activity.splitName)) {
                ResolveInfo installer = packages.getInstantAppInstallerInfo();
                if (installer.activityInfo == null) { infos.remove(i); continue; }
                if (blockInstant && packages.isInstantAppInternal(activity.packageName, user, 1000)) {
                    infos.remove(i); continue;
                }
                var replacement = new ResolveInfo(installer);
                replacement.auxiliaryInfo = new AuxiliaryResolveInfo(failure(activity.packageName, filterUid, user),
                        activity.packageName, activity.applicationInfo.longVersionCode, activity.splitName);
                replacement.filter = new IntentFilter();
                replacement.resolvePackageName = info.getComponentInfo().packageName;
                replacement.labelRes = info.resolveLabelResId(); replacement.icon = info.resolveIconResId();
                replacement.isInstantAppAvailable = true; infos.set(i, replacement); continue;
            }
            if (ephemeral == null) {
                if (forStart || !packages.shouldFilterApplication(packages.getPackageStateInternal(activity.packageName),
                        filterUid, user, false)) continue;
            } else if (ephemeral.equals(activity.packageName)) continue;
            else if (forStart && (intent.isWebIntent() || (intent.getFlags() & 0x800) != 0)
                    && intent.getPackage() == null && intent.getComponent() == null) continue;
            else if ((activity.flags & 0x100000) != 0 && !activity.applicationInfo.isInstantApp()) continue;
            infos.remove(i);
        }
        return infos;
    }
    private ComponentName failure(String name, int filterUid, int user) {
        Intent intent = new Intent("android.intent.action.INSTALL_FAILURE"); intent.setPackage(name);
        var result = packages.queryIntentActivitiesInternal(intent, null, 0, 0, filterUid, -1, user, false, false);
        for (ResolveInfo info : result) if (info.activityInfo.splitName == null)
            return new ComponentName(name, info.activityInfo.name);
        return null;
    }
    private static boolean contains(String[] values, String value) {
        if (values != null) for (String candidate : values) if (Objects.equals(candidate, value)) return true;
        return false;
    }
}
