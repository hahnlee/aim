/* Copyright The Android Open Source Project. Apache License 2.0.
 * PackageManagerService/BroadcastHelper behavior from android-16.0.0_r1.
 */
package com.android.server.pm;

import android.app.ActivityManager;
import android.app.ActivityManagerInternal;
import android.app.BroadcastOptions;
import android.app.admin.DevicePolicyManagerInternal;
import android.app.admin.IDevicePolicyManager;
import android.app.role.RoleManager;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageInstaller;
import android.content.pm.PackageManager;
import android.content.pm.UserInfo;
import android.content.pm.UserProperties;
import android.net.Uri;
import android.os.Binder;
import android.os.Bundle;
import android.os.Handler;
import android.os.IBinder;
import android.os.IRemoteCallback;
import android.os.Process;
import android.os.RemoteException;
import android.os.ServiceManager;
import android.os.SystemClock;
import android.os.UserHandle;
import android.provider.DeviceConfig;
import android.util.SparseArray;
import com.android.server.LocalServices;
import com.android.server.apphibernation.AppHibernationManagerInternal;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.PackageStateInternal;
import com.android.server.pm.pkg.SuspendParams;
import dev.aim.server.IPackageMutationBridge;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Objects;
import java.util.function.BiFunction;
import java.util.function.Supplier;

/** Real AM/UM/ProtectedPackages/PackageMonitor owners; no PMS instance. */
public final class NativePackageEffectsBridge extends IPackageMutationBridge.Stub {
    /** Image policy values, read from the pinned original compiled owners. */
    public record Policy(boolean reduceComponentBroadcasts, boolean privateProfileAccess,
            boolean stayStopped, long serviceStartWithDelay) {}
    private final Context context;
    private final Handler handler;
    private final Supplier<Computer> snapshots;
    private final Policy policy;
    private final ActivityManagerInternal activity;
    private final UserManagerInternal users;
    private final ProtectedPackages protectedPackages;
    private final PackageMonitorCallbackHelper monitors;
    private final LinkedHashMap<Integer, LinkedHashMap<String, ArrayList<String>>> pending = new LinkedHashMap<>();
    private volatile boolean closed;
    private final java.util.HashSet<Runnable> scheduled = new java.util.HashSet<>();
    private boolean pendingScheduled;
    private int pendingCallingUid;

    public NativePackageEffectsBridge(Context context, Handler handler,
            Supplier<Computer> snapshots, Policy policy, ProtectedPackages protectedPackages) {
        this.context = Objects.requireNonNull(context);
        this.handler = Objects.requireNonNull(handler);
        this.snapshots = Objects.requireNonNull(snapshots);
        this.policy = Objects.requireNonNull(policy);
        activity = Objects.requireNonNull(LocalServices.getService(ActivityManagerInternal.class), "missing ActivityManagerInternal");
        users = Objects.requireNonNull(LocalServices.getService(UserManagerInternal.class), "missing UserManagerInternal");
        this.protectedPackages = Objects.requireNonNull(protectedPackages);
        monitors = new PackageMonitorCallbackHelper();
    }
    public void close() {
        closed = true;
        synchronized (scheduled) { for (Runnable callback : scheduled) handler.removeCallbacks(callback); scheduled.clear(); }
        synchronized (pending) { pending.clear(); pendingScheduled = false; }
        monitors.onUserRemoved(-1);
        for (int user : users.getUserIds()) monitors.onUserRemoved(user);
    }
    private void post(Runnable action) { postDelayed(action, 0); }
    private void postDelayed(Runnable action, long delay) {
        Runnable[] callback = new Runnable[1];
        callback[0] = () -> {
            synchronized (scheduled) { scheduled.remove(callback[0]); }
            if (!closed) action.run();
        };
        synchronized (scheduled) {
            if (closed) throw new IllegalStateException("package effects owner closed");
            scheduled.add(callback[0]);
            if (!handler.postDelayed(callback[0], delay)) {
                scheduled.remove(callback[0]);
                throw new IllegalStateException("package effects handler stopped");
            }
        }
    }
    public ProtectedPackages protectedPackages() { return protectedPackages; }
    public PackageMonitorCallbackHelper packageMonitors() { return monitors; }
    private static void enforceNative() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID) throw new SecurityException("native package effects require system UID");
    }
    private Computer snapshot() { return Objects.requireNonNull(snapshots.get(), "native Computer unavailable"); }
    private void clean(Runnable action) {
        long identity = Binder.clearCallingIdentity();
        try { action.run(); } finally { Binder.restoreCallingIdentity(identity); }
    }
    @Override public boolean requireInstallerPermission(int callingUid) {
        enforceNative();
        android.util.EventLog.writeEvent(0x534e4554, "150857253", callingUid, "");
        long identity = Binder.clearCallingIdentity();
        try {
            var compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(ServiceManager.getService("platform_compat"));
            if (compat == null) throw new IllegalStateException("missing PlatformCompat");
            return compat.isChangeEnabledByUid(150857253L, callingUid);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        finally { Binder.restoreCallingIdentity(identity); }
    }
    @Override public void killPackage(String name, int appId, int user, String reason, int exitReason) {
        enforceNative();
        clean(() -> {
            try { Objects.requireNonNull(ActivityManager.getService(), "missing ActivityManager").killApplication(name, appId, user, reason, exitReason); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        });
    }
    @Override public boolean isStateProtected(String name, int user) { enforceNative(); return protectedPackages.isPackageStateProtected(user, name); }
    @Override public boolean isDataProtected(String name, int user) { enforceNative(); return protectedPackages.isPackageDataProtected(user, name); }
    @Override public String getOwnerPackage(int user) { enforceNative(); return protectedPackages.getDeviceOwnerOrProfileOwnerPackage(user); }
    @Override public void setDeviceAndProfileOwnerPackages(int ownerUser, String ownerPackage, int[] profileUsers, String[] profilePackages) {
        enforceNative();
        if (profileUsers.length != profilePackages.length) throw new IllegalArgumentException("profile owner inventory differs");
        SparseArray<String> profiles = new SparseArray<>();
        for (int i = 0; i < profileUsers.length; i++) profiles.put(profileUsers[i], profilePackages[i]);
        protectedPackages.setDeviceAndProfileOwnerPackages(ownerUser, ownerPackage, profiles);
    }
    @Override public void setOwnerProtectedPackages(int user, String[] packages) {
        enforceNative(); protectedPackages.setOwnerProtectedPackages(user, packages == null ? null : Arrays.asList(packages));
    }
    @Override public boolean isActiveDeviceAdmin(String name, int user) {
        enforceNative();
        IDevicePolicyManager dpm = IDevicePolicyManager.Stub.asInterface(ServiceManager.getService("device_policy"));
        DevicePolicyManagerInternal internal = LocalServices.getService(DevicePolicyManagerInternal.class);
        if (dpm == null || internal == null) return false; // Original early-bootstrap null-owner policy.
        try {
            ComponentName deviceOwner = dpm.getDeviceOwnerComponent(false);
            if (name.equals(deviceOwner == null ? null : deviceOwner.getPackageName())) return true;
            int[] allUsers = users.getUserIds();
            int[] targets = user == -1 ? allUsers : new int[] {user};
            for (int target : targets) if (dpm.packageHasActiveAdmins(name, target)) return true;
            PackageStateInternal state = snapshot().getPackageStateInternal(name);
            if (state == null) return false;
            RoleManager roles = Objects.requireNonNull(context.getSystemService(RoleManager.class));
            for (int target : state.isSystem() ? allUsers : targets) {
                List<String> holders = roles.getRoleHoldersAsUser(RoleManager.ROLE_DEVICE_POLICY_MANAGEMENT, UserHandle.of(target));
                String holder = holders.isEmpty() ? null : holders.get(0);
                if (Objects.equals(name, holder) && internal.isUserOrganizationManaged(target)) return true;
            }
            return false;
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    private void deliver(String action, String name, Bundle extras, int flags, String target,
            int[] userIds, int[] instantUsers, SparseArray<int[]> allowLists,
            BiFunction<Integer, Bundle, Bundle> filter, Bundle options) {
        int[] targets = instantUsers != null && instantUsers.length != 0 ? instantUsers : userIds;
        boolean instant = instantUsers != null && instantUsers.length != 0;
        if (targets == null) {
            try { targets = Objects.requireNonNull(ActivityManager.getService()).getRunningUserIds(); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
        for (int user : targets) {
            Intent intent = new Intent(action, name == null ? null : Uri.fromParts("package", name, null));
            if (extras != null) intent.putExtras(extras);
            if (target != null) intent.setPackage(target);
            int uid = intent.getIntExtra(Intent.EXTRA_UID, -1);
            if (uid >= 0 && UserHandle.getUserId(uid) != user) intent.putExtra(Intent.EXTRA_UID, UserHandle.getUid(user, UserHandle.getAppId(uid)));
            if (allowLists != null && "android".equals(target)) intent.putExtra(Intent.EXTRA_VISIBILITY_ALLOW_LIST, allowLists.get(user));
            intent.putExtra(Intent.EXTRA_USER_HANDLE, user);
            intent.addFlags(Intent.FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT | flags);
            activity.broadcastIntentWithCallback(intent, null, instant ? new String[] {"android.permission.ACCESS_INSTANT_APPS"} : null,
                    user, instant || allowLists == null ? null : allowLists.get(user), instant ? null : filter, options);
        }
    }
    private void sendAndNotify(String action, String name, Bundle extras, int flags, String target,
            int[] userIds, int[] instantUsers, SparseArray<int[]> allowLists) {
        post(() -> deliver(action, name, extras, flags, target, userIds, instantUsers, allowLists, null, null));
        if (target == null) monitors.notifyPackageMonitor(action, name, extras, userIds, instantUsers, allowLists, handler, null);
    }
    @Override public void packageChanged(String name, int uid, boolean dontKill, String[] components,
            String reason, int callingUid, boolean delayed) {
        enforceNative();
        if (delayed) {
            synchronized (pending) {
                ArrayList<String> values = pending.computeIfAbsent(UserHandle.getUserId(uid), ignored -> new LinkedHashMap<>()).computeIfAbsent(name, ignored -> new ArrayList<>());
                for (String component : components) if (!values.contains(component)) values.add(component);
                if (!pendingScheduled) {
                    pendingScheduled = true;
                    pendingCallingUid = callingUid;
                    postDelayed(this::flushPending, SystemClock.uptimeMillis() > policy.serviceStartWithDelay() ? 1000 : 10000);
                }
            }
        } else {
            synchronized (pending) { if (pending.containsKey(UserHandle.getUserId(uid))) pending.get(UserHandle.getUserId(uid)).remove(name); }
            clean(() -> changedNow(name, uid, dontKill, new ArrayList<>(Arrays.asList(components)), reason, callingUid));
        }
    }
    private void flushPending() {
        LinkedHashMap<Integer, LinkedHashMap<String, ArrayList<String>>> values;
        int callingUid;
        synchronized (pending) { values = new LinkedHashMap<>(pending); pending.clear(); pendingScheduled = false; callingUid = pendingCallingUid; }
        Computer snapshot = snapshot();
        ArrayList<Integer> orderedUsers = new ArrayList<>(values.keySet());
        orderedUsers.sort(Integer::compare);
        for (int userId : orderedUsers) {
            ArrayList<String> orderedPackages = new ArrayList<>(values.get(userId).keySet());
            orderedPackages.sort(java.util.Comparator.comparingInt(String::hashCode));
            for (String packageName : orderedPackages) {
                var entry = java.util.Map.entry(packageName, values.get(userId).get(packageName));
            PackageStateInternal state = snapshot.getPackageStateInternal(entry.getKey());
            if (state != null) changedNow(entry.getKey(), UserHandle.getUid(userId, state.getAppId()), true, entry.getValue(), null, callingUid);
            }
        }
    }
    private void changedNow(String name, int uid, boolean dontKill, ArrayList<String> components, String reason, int callingUid) {
        Computer snapshot = snapshot();
        PackageStateInternal state = snapshot.getPackageStateInternal(name, Process.SYSTEM_UID);
        if (state == null || state.getPkg() == null) return;
        int user = UserHandle.getUserId(uid);
        boolean instant = snapshot.isInstantAppInternal(name, user, Process.SYSTEM_UID);
        int[] normalUsers = instant ? new int[0] : new int[] {user};
        int[] instantUsers = instant ? new int[] {user} : new int[0];
        SparseArray<int[]> allowLists = instant ? null : snapshot.getVisibilityAllowLists(name, normalUsers);
        String[] shared = snapshot.getSharedUserPackagesForPackage(name, user);
        post(() -> {
            if (components.contains(name) || !policy.reduceComponentBroadcasts()) {
                changedDelivery(name, uid, dontKill, components, reason, null, normalUsers, instantUsers, allowLists);
                return;
            }
            ArrayList<String> privateComponents = new ArrayList<>();
            AndroidPackage code = state.getPkg();
            for (var component : code.getReceivers()) if (components.contains(component.getClassName()) && !component.isExported()) privateComponents.add(component.getClassName());
            for (var component : code.getProviders()) if (components.contains(component.getClassName()) && !component.isExported()) privateComponents.add(component.getClassName());
            for (var component : code.getServices()) if (components.contains(component.getClassName()) && !component.isExported()) privateComponents.add(component.getClassName());
            for (var component : code.getActivities()) if (components.contains(component.getClassName()) && !component.isExported()) privateComponents.add(component.getClassName());
            ArrayList<String> exported = new ArrayList<>(components);
            exported.removeAll(privateComponents);
            if (!privateComponents.isEmpty()) {
                if (!"android".equals(name)) changedDelivery(name, uid, dontKill, privateComponents, reason, "android", normalUsers, instantUsers, allowLists);
                changedDelivery(name, uid, dontKill, privateComponents, reason, name, normalUsers, instantUsers, allowLists);
                for (String peer : shared) if (!name.equals(peer)) changedDelivery(name, uid, dontKill, privateComponents, reason, peer, normalUsers, instantUsers, allowLists);
            }
            if (!exported.isEmpty()) changedDelivery(name, uid, dontKill, exported, reason, null, normalUsers, instantUsers, allowLists);
        });
        monitors.notifyPackageChanged(name, dontKill, components, uid, reason, normalUsers, instantUsers, allowLists, handler);
    }
    private void changedDelivery(String name, int uid, boolean dontKill, ArrayList<String> components,
            String reason, String target, int[] normal, int[] instant, SparseArray<int[]> allowLists) {
        Bundle extras = new Bundle(4);
        extras.putString(Intent.EXTRA_CHANGED_COMPONENT_NAME, components.get(0));
        extras.putStringArray(Intent.EXTRA_CHANGED_COMPONENT_NAME_LIST, components.toArray(new String[0]));
        extras.putBoolean(Intent.EXTRA_DONT_KILL_APP, dontKill);
        extras.putInt(Intent.EXTRA_UID, uid);
        if (reason != null) extras.putString(Intent.EXTRA_REASON, reason);
        deliver(Intent.ACTION_PACKAGE_CHANGED, name, extras, components.contains(name) ? 0 : Intent.FLAG_RECEIVER_REGISTERED_ONLY,
                target, normal, instant, allowLists, null, null);
    }
    @Override public void componentLabelChanged(String name, int uid, String component, int callingUid) {
        enforceNative();
        synchronized (pending) {
            ArrayList<String> values = pending.computeIfAbsent(UserHandle.getUserId(uid), ignored -> new LinkedHashMap<>()).computeIfAbsent(name, ignored -> new ArrayList<>());
            if (!values.contains(component)) values.add(component);
            if (!pendingScheduled) {
                pendingScheduled = true;
                pendingCallingUid = callingUid;
                postDelayed(this::flushPending, 1000);
            }
        }
    }
    @Override public void firstLaunch(String name, String installer, int user) {
        enforceNative();
        clean(() -> {
            boolean instant = snapshot().isInstantAppInternal(name, user, Process.SYSTEM_UID);
            post(() -> deliver(Intent.ACTION_PACKAGE_FIRST_LAUNCH, name, null, 0, installer,
                    instant ? new int[0] : new int[] {user}, instant ? new int[] {user} : new int[0], null, null, null));
        });
    }
    @Override public void unhibernate(String name, int user) {
        enforceNative();
        post(() -> {
            AppHibernationManagerInternal hibernation = LocalServices.getService(AppHibernationManagerInternal.class);
            if (hibernation != null && hibernation.isHibernatingForUser(name, user)) {
                hibernation.setHibernatingForUser(name, user, false);
                hibernation.setHibernatingGlobally(name, false);
            }
        });
    }
    @Override public void packageUnstopped(String name, int user) {
        enforceNative();
        if (!policy.stayStopped()) return;
        Computer snapshot = snapshot();
        Bundle extras = new Bundle();
        extras.putInt(Intent.EXTRA_UID, snapshot.getPackageUid(name, 0, user));
        extras.putInt(Intent.EXTRA_USER_HANDLE, user);
        extras.putLong(Intent.EXTRA_TIME, SystemClock.elapsedRealtime());
        sendAndNotify(Intent.ACTION_PACKAGE_UNSTOPPED, name, extras, Intent.FLAG_RECEIVER_REGISTERED_ONLY,
                null, new int[] {user}, null, snapshot.getVisibilityAllowLists(name, new int[] {user}));
    }
    @Override public void packageHidden(String name, int user, boolean hidden) {
        enforceNative();
        clean(() -> {
            PackageStateInternal state = Objects.requireNonNull(snapshot().getPackageStateInternal(name));
            if (hidden) {
                killPackage(name, state.getAppId(), user, "hiding pkg", 13 /* REASON_OTHER */);
                Bundle extras = new Bundle();
                extras.putInt(Intent.EXTRA_UID, UserHandle.getUid(user, state.getAppId()));
                extras.putBoolean(Intent.EXTRA_DATA_REMOVED, false);
                extras.putBoolean(Intent.EXTRA_SYSTEM_UPDATE_UNINSTALL, false);
                extras.putBoolean(Intent.EXTRA_DONT_KILL_APP, false);
                extras.putBoolean(Intent.EXTRA_USER_INITIATED, true);
                extras.putBoolean(Intent.EXTRA_REMOVED_FOR_ALL_USERS, false);
                String installer = state.getInstallSource().mInstallerPackageName;
                if (installer != null) sendAndNotify(Intent.ACTION_PACKAGE_REMOVED, name, extras, 0, installer, new int[] {user}, null, null);
                sendAndNotify(Intent.ACTION_PACKAGE_REMOVED, name, extras, 0, null, new int[] {user}, null, null);
                sendAndNotify(Intent.ACTION_PACKAGE_REMOVED_INTERNAL, name, extras, 0, "android", new int[] {user}, null, null);
            } else added(name, user, false, 0, null);
        });
    }
    @Override public void packageAdded(String name, int user, boolean archived, int dataLoaderType, String predictionPackage) {
        enforceNative(); clean(() -> added(name, user, archived, dataLoaderType, predictionPackage));
    }
    private void added(String name, int user, boolean archived, int dataLoaderType, String predictionPackage) {
        Computer snapshot = snapshot();
        PackageStateInternal state = Objects.requireNonNull(snapshot.getPackageStateInternal(name));
        boolean instant = state.getUserStateOrDefault(user).isInstantApp();
        int[] normal = instant ? new int[0] : new int[] {user};
        int[] instantUsers = instant ? new int[] {user} : new int[0];
        SparseArray<int[]> allowLists = snapshot.getVisibilityAllowLists(name, normal);
        Bundle extras = new Bundle(1);
        extras.putInt(Intent.EXTRA_UID, UserHandle.getUid(user, state.getAppId()));
        if (archived) extras.putBoolean(Intent.EXTRA_ARCHIVAL, true);
        extras.putInt(PackageInstaller.EXTRA_DATA_LOADER_TYPE, dataLoaderType);
        post(() -> {
            deliver(Intent.ACTION_PACKAGE_ADDED, name, extras, 0, null, normal, instantUsers, allowLists, null, null);
            PackageManager packages = context.getPackageManager();
            if (DeviceConfig.getBoolean("privacy", "safety_label_change_notifications_enabled", true)
                    && !packages.hasSystemFeature("android.hardware.type.automotive")
                    && !packages.hasSystemFeature("android.software.leanback")
                    && !packages.hasSystemFeature("android.hardware.type.watch")) {
                deliver(Intent.ACTION_PACKAGE_ADDED, name, extras, 0, packages.getPermissionControllerPackageName(), normal, instantUsers, allowLists, null, null);
            }
        });
        monitors.notifyPackageAddedForNewUsers(name, state.getAppId(), normal, instantUsers, archived, dataLoaderType, allowLists, handler);
        if (state.isSystem() && !instant) post(() -> bootCompleted(name, user));
        UserManagerService service = Objects.requireNonNull(UserManagerService.getInstance(), "missing original UserManagerService");
        UserInfo parent = service.getProfileParent(user);
        int launcherUser = parent == null ? user : parent.id;
        ComponentName launcher = snapshot.getDefaultHomeActivity(launcherUser);
        PackageInstaller.SessionInfo session = new PackageInstaller.SessionInfo();
        session.installReason = state.getUserStateOrDefault(user).getInstallReason();
        session.appPackageName = name;
        boolean access = !policy.privateProfileAccess()
                || users.getUserProperties(user).getProfileApiVisibility() != UserProperties.PROFILE_API_VISIBILITY_HIDDEN
                || launcher != null && (context.getPackageManager().checkPermission("android.permission.ACCESS_HIDDEN_PROFILES_FULL", launcher.getPackageName()) == 0
                || context.getPackageManager().checkPermission("android.permission.ACCESS_HIDDEN_PROFILES", launcher.getPackageName()) == 0);
        if (launcher != null && access) context.sendBroadcastAsUser(new Intent(PackageInstaller.ACTION_SESSION_COMMITTED)
                .putExtra(PackageInstaller.EXTRA_SESSION, session).putExtra(Intent.EXTRA_USER, UserHandle.of(user)).setPackage(launcher.getPackageName()), UserHandle.of(launcherUser));
        if (predictionPackage != null) context.sendBroadcastAsUser(new Intent(PackageInstaller.ACTION_SESSION_COMMITTED)
                .putExtra(PackageInstaller.EXTRA_SESSION, session).putExtra(Intent.EXTRA_USER, UserHandle.of(user)).setPackage(predictionPackage), UserHandle.of(launcherUser));
    }
    private void bootCompleted(String name, int user) {
        if (!users.isUserRunning(user)) return;
        BroadcastOptions options = BroadcastOptions.makeBasic();
        options.setTemporaryAppAllowlist(activity.getBootTimeTempAllowListDuration(), android.os.PowerExemptionManager.TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED, android.os.PowerExemptionManager.REASON_LOCKED_BOOT_COMPLETED, "");
        try {
            var manager = Objects.requireNonNull(ActivityManager.getService());
            Intent locked = new Intent(Intent.ACTION_LOCKED_BOOT_COMPLETED).setPackage(name).putExtra(Intent.EXTRA_USER_HANDLE, user);
            manager.broadcastIntentWithFeature(null, null, locked, null, null, 0, null, null,
                    new String[] {"android.permission.RECEIVE_BOOT_COMPLETED"}, null, null, -1, options.toBundle(), false, false, user);
            if (users.isUserUnlockingOrUnlocked(user)) manager.broadcastIntentWithFeature(null, null,
                    new Intent(Intent.ACTION_BOOT_COMPLETED).setPackage(name).putExtra(Intent.EXTRA_USER_HANDLE, user), null, null, 0, null, null,
                    new String[] {"android.permission.RECEIVE_BOOT_COMPLETED"}, null, null, -1, options.toBundle(), false, false, user);
        } catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    private Bundle filterChangedPackages(int callingUid, Bundle extras) {
        if (UserHandle.isCore(callingUid)) return extras;
        String[] packages = extras.getStringArray(Intent.EXTRA_CHANGED_PACKAGE_LIST);
        if (packages == null || packages.length == 0) return extras;
        int[] uids = extras.getIntArray(Intent.EXTRA_CHANGED_UID_LIST);
        int user = extras.getInt(Intent.EXTRA_USER_HANDLE, UserHandle.getUserId(callingUid));
        Computer snapshot = snapshot();
        ArrayList<String> visible = new ArrayList<>();
        ArrayList<Integer> visibleUids = new ArrayList<>();
        for (int i = 0; i < packages.length; i++) {
            if (snapshot.shouldFilterApplication(snapshot.getPackageStateInternal(packages[i]), callingUid, user)) continue;
            visible.add(packages[i]);
            if (uids != null && i < uids.length) visibleUids.add(uids[i]);
        }
        if (visible.isEmpty()) return null;
        Bundle result = new Bundle(extras);
        result.putStringArray(Intent.EXTRA_CHANGED_PACKAGE_LIST, visible.toArray(new String[0]));
        if (uids != null) {
            int[] values = new int[visibleUids.size()];
            for (int i = 0; i < values.length; i++) values[i] = visibleUids.get(i);
            result.putIntArray(Intent.EXTRA_CHANGED_UID_LIST, values);
        }
        return result;
    }
    private boolean callerOwnsUser(int user, int uid) {
        if (uid == Process.SYSTEM_UID) return true;
        String owner = protectedPackages.getDeviceOwnerOrProfileOwnerPackage(user);
        return owner != null && snapshot().getPackageUidInternal(owner, 0, user, uid) == uid;
    }
    @Override public boolean isSuspensionAllowed(int user, int uid) {
        enforceNative();
        return callerOwnsUser(user, uid) || (!users.hasUserRestriction("no_control_apps", user)
                && !users.hasUserRestriction("no_uninstall_apps", user));
    }
    @Override public boolean[] canSuspendPackages(String[] names, int user, int uid) {
        enforceNative();
        boolean[] result = new boolean[names.length];
        Computer snapshot = snapshot();
        boolean owner = callerOwnsUser(user, uid);
        long identity = Binder.clearCallingIdentity();
        try {
            ComponentName home = snapshot.getDefaultHomeActivity(user);
            List<String> dialers = Objects.requireNonNull(context.getSystemService(RoleManager.class))
                    .getRoleHoldersAsUser(RoleManager.ROLE_DIALER, UserHandle.of(user));
            String dialer = dialers.isEmpty() ? null : dialers.get(0);
            android.content.pm.PackageManagerInternal internal = Objects.requireNonNull(LocalServices.getService(android.content.pm.PackageManagerInternal.class));
            ArrayList<String> required = new ArrayList<>();
            for (int role : new int[] {2, 3, 4, 7}) {
                String[] packages = internal.getKnownPackageNames(role, user);
                if (packages.length != 0) required.add(packages[0]);
            }
            android.app.AppOpsManager appOps = Objects.requireNonNull(context.getSystemService(android.app.AppOpsManager.class));
            for (int i = 0; i < names.length; i++) {
                String name = Objects.requireNonNull(names[i]);
                if (isActiveDeviceAdmin(name, user) || name.equals(home == null ? null : home.getPackageName())
                        || name.equals(dialer) || required.contains(name) || protectedPackages.isPackageStateProtected(user, name)
                        || !owner && snapshot.getBlockUninstall(user, name) || "android".equals(name)) continue;
                PackageStateInternal state = snapshot.getPackageStateInternal(name);
                AndroidPackage code = state == null ? null : state.getPkg();
                if (code != null && (code.isSdkLibrary() || code.isStaticSharedLibrary()
                        || appOps.checkOpNoThrow(android.app.AppOpsManager.OP_SYSTEM_EXEMPT_FROM_SUSPENSION,
                                UserHandle.getUid(user, state.getAppId()), name) == 0)) continue;
                result[i] = true;
            }
            return result;
        } finally { Binder.restoreCallingIdentity(identity); }
    }
    @Override public void packagesSuspended(String[] names, int[] uids, int user, boolean suspended, boolean quarantined, boolean changedOnly) {
        enforceNative();
        clean(() -> {
            Bundle extras = new Bundle(3);
            extras.putStringArray(Intent.EXTRA_CHANGED_PACKAGE_LIST, names);
            extras.putIntArray(Intent.EXTRA_CHANGED_UID_LIST, uids);
            if (quarantined) extras.putBoolean(Intent.EXTRA_QUARANTINED, true);
            String action = changedOnly ? Intent.ACTION_PACKAGES_SUSPENSION_CHANGED : suspended ? Intent.ACTION_PACKAGES_SUSPENDED : Intent.ACTION_PACKAGES_UNSUSPENDED;
            Bundle options = new BroadcastOptions().setDeferralPolicy(BroadcastOptions.DEFERRAL_POLICY_UNTIL_ACTIVE).toBundle();
            post(() -> deliver(action, null, extras, Intent.FLAG_RECEIVER_REGISTERED_ONLY | Intent.FLAG_RECEIVER_FOREGROUND,
                    null, new int[] {user}, null, null, this::filterChangedPackages, options));
            monitors.notifyPackageMonitor(action, null, extras, new int[] {user}, null, null, handler, this::filterChangedPackages);
            if (!changedOnly) post(() -> {
                Computer snapshot = snapshot();
                for (String name : names) {
                    Bundle all = new Bundle();
                    PackageStateInternal state = snapshot.getPackageStateInternal(name, Process.SYSTEM_UID);
                    if (suspended && state != null) {
                        var parameters = state.getUserStateOrDefault(user).getSuspendParams();
                        if (parameters != null) for (int i = 0; i < parameters.size(); i++) {
                            SuspendParams parameter = parameters.valueAt(i);
                            if (parameter != null && parameter.getAppExtras() != null) all.putAll(parameter.getAppExtras());
                        }
                    }
                    Bundle appExtras = null;
                    if (all.size() != 0) { appExtras = new Bundle(1); appExtras.putBundle(Intent.EXTRA_SUSPENDED_PACKAGE_EXTRAS, all); }
                    deliver(suspended ? Intent.ACTION_MY_PACKAGE_SUSPENDED : Intent.ACTION_MY_PACKAGE_UNSUSPENDED,
                            null, appExtras, Intent.FLAG_RECEIVER_INCLUDE_BACKGROUND, name, new int[] {user}, null, null, null, null);
                }
            });
        });
    }
    @Override public void removedSuspensions(String[] names, int[] uids, int user) {
        enforceNative();
        post(() -> {
            for (String name : names) deliver(Intent.ACTION_MY_PACKAGE_UNSUSPENDED, null, null,
                    Intent.FLAG_RECEIVER_INCLUDE_BACKGROUND, name, new int[] {user}, null, null, null, null);
        });
        Bundle extras = new Bundle(3);
        extras.putStringArray(Intent.EXTRA_CHANGED_PACKAGE_LIST, names);
        extras.putIntArray(Intent.EXTRA_CHANGED_UID_LIST, uids);
        Bundle options = new BroadcastOptions().setDeferralPolicy(BroadcastOptions.DEFERRAL_POLICY_UNTIL_ACTIVE).toBundle();
        post(() -> deliver(Intent.ACTION_PACKAGES_UNSUSPENDED, null, extras,
                Intent.FLAG_RECEIVER_REGISTERED_ONLY | Intent.FLAG_RECEIVER_FOREGROUND,
                null, new int[] {user}, null, null, this::filterChangedPackages, options));
        monitors.notifyPackageMonitor(Intent.ACTION_PACKAGES_UNSUSPENDED, null, extras,
                new int[] {user}, null, null, handler, this::filterChangedPackages);
    }
    @Override public void distractionChanged(String[] names, int[] uids, int user, int flags) {
        enforceNative();
        Bundle extras = new Bundle();
        extras.putStringArray(Intent.EXTRA_CHANGED_PACKAGE_LIST, names);
        extras.putIntArray(Intent.EXTRA_CHANGED_UID_LIST, uids);
        extras.putInt(Intent.EXTRA_DISTRACTION_RESTRICTIONS, flags);
        post(() -> deliver(Intent.ACTION_DISTRACTING_PACKAGES_CHANGED, null, extras, Intent.FLAG_RECEIVER_REGISTERED_ONLY,
                null, new int[] {user}, null, null, this::filterChangedPackages, null));
    }
    @Override public void registerPackageMonitor(IBinder callback, int user, int uid) {
        enforceNative(); monitors.registerPackageMonitorCallback(Objects.requireNonNull(IRemoteCallback.Stub.asInterface(callback)), user, uid);
    }
    @Override public void unregisterPackageMonitor(IBinder callback) { enforceNative(); monitors.unregisterPackageMonitorCallback(IRemoteCallback.Stub.asInterface(callback)); }
    @Override public void userRemoved(int user) {
        enforceNative();
        synchronized (pending) { pending.remove(user); }
        monitors.onUserRemoved(user);
    }
}
