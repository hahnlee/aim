package com.android.server.pm;

import android.content.Context;
import android.content.Intent;
import android.content.IntentSender;
import android.content.ComponentName;
import android.content.pm.ArchivedPackageParcel;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.os.Binder;
import android.os.Handler;
import android.os.RemoteException;
import android.os.UserHandle;
import com.android.server.pm.pkg.ArchiveState;
import dev.aim.server.IPackageInternalHost;
import dev.aim.server.PackageSnapshots;
import java.util.Objects;
import java.util.Set;
import java.util.function.Supplier;

/** Original archiver type; every virtual PMS-dependent entry is owned by native producers. */
public final class NativePackageArchiver extends PackageArchiver {
    private final Context context;
    private final PackageSnapshots.Store packages;
    private final IPackageInternalHost host;
    private final Supplier<NativeComputer> computers;
    private final Handler handler;
    private final java.util.Map<android.util.Pair<Integer,String>,IntentSender> listeners = new java.util.HashMap<>();
    public NativePackageArchiver(Context context, PackageSnapshots.Store packages,
            IPackageInternalHost host, Supplier<NativeComputer> computers, Handler handler) {
        // The original constructor initializes only independent context/graphics helpers.
        // Its private PMS slot is unreachable: all PMS-dependent virtual entries are overridden.
        super(Objects.requireNonNull(context), null);
        this.context = context; this.packages = Objects.requireNonNull(packages);
        this.host = Objects.requireNonNull(host); this.computers = Objects.requireNonNull(computers);
        this.handler = Objects.requireNonNull(handler);
    }
    @Override void requestArchive(String name, String caller, IntentSender receiver, UserHandle user) {
        requestArchive(name, caller, 0, receiver, user);
    }
    @Override void requestArchive(String name, String caller, int flags, IntentSender receiver, UserHandle user) {
        Objects.requireNonNull(name); Objects.requireNonNull(caller); Objects.requireNonNull(receiver); Objects.requireNonNull(user);
        try { host.archivePackage(name, caller, flags, receiver, user.getIdentifier(), Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override void requestUnarchive(String name, String caller, IntentSender receiver, UserHandle user) {
        Objects.requireNonNull(name); Objects.requireNonNull(caller); Objects.requireNonNull(receiver); Objects.requireNonNull(user);
        try { host.unarchivePackage(name, caller, receiver, user.getIdentifier(), false, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override public boolean isIntentResolvedToArchivedApp(Intent intent, int user) {
        String name = packageName(intent);
        if (name == null || intent.getComponent() == null) return false;
        try (var scope = packages.computer()) {
            var state = scope.getPackageStateInternal(name);
            if (state == null) return false;
            var userState = state.getUserStateOrDefault(user);
            if (userState.isInstalled() || userState.getArchiveState() == null) return false;
            for (var activity : userState.getArchiveState().getActivityInfos())
                if (activity.getOriginalComponentName().equals(intent.getComponent())) return true;
            return false;
        }
    }
    @Override void clearArchiveState(String name, int user) {
        try { host.clearArchiveState(name, user, Binder.getCallingUid(), Binder.getCallingPid()); }
        catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }
    @Override void clearArchiveState(PackageSetting setting, int user) {
        if (setting != null) clearArchiveState(setting.getPackageName(), user);
    }
    @Override public boolean verifySupportsUnarchival(String installer, int user) {
        long identity = Binder.clearCallingIdentity();
        try (var scope = packages.computer()) {
            Intent intent = new Intent("android.intent.action.UNARCHIVE_PACKAGE").setPackage(installer);
            return !scope.queryIntentReceiversInternal(intent, null, 0, user, Binder.getCallingUid(),
                    Binder.getCallingPid(), false).isEmpty();
        } finally { Binder.restoreCallingIdentity(identity); }
    }
    @Override public boolean isAppArchivable(String name, UserHandle user) {
        try (var scope = packages.computer()) {
            try { return scope.readQueries().isAppArchivable(name, user); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
    }
    @Override public Bitmap getArchivedAppIcon(String name, UserHandle user, String caller) {
        try (var scope = packages.computer()) {
            try { return scope.readQueries().getArchivedAppIcon(name, user, caller); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
    }
    @Override ArchiveState createArchiveState(ArchivedPackageParcel archived, int user,
            String installer, String title) {
        try (var scope = packages.computer()) {
            if (scope.getApplicationInfoInternal(installer, 0, Binder.getCallingUid(), user) == null || title == null)
                return null;
        }
        return NativeArchiveGraphics.store(context, archived, user, title);
    }
    private static String packageName(Intent intent) {
        if (intent == null) return null;
        ComponentName component = intent.getComponent();
        return component == null ? intent.getPackage() : component.getPackageName();
    }
    @Override public int requestUnarchiveOnActivityStart(Intent intent, String caller, int user, int callerUid) {
        String name = packageName(intent);
        if (name == null || caller == null) return -92;
        if (!qualified(caller, callerUid, user)) return -94;
        try {
            var appOps = Objects.requireNonNull(context.getSystemService(android.app.AppOpsManager.class));
            boolean confirmation = appOps.checkOp(android.app.AppOpsManager.OP_UNARCHIVAL_CONFIRMATION, callerUid, caller) == 0;
            if (confirmation) {
                var session = host.getActiveUnarchiveSession(name, user, Binder.getCallingUid(), Binder.getCallingPid());
                if (session != null) {
                    handler.post(() -> Objects.requireNonNull(context.getSystemService(android.content.pm.LauncherApps.class))
                            .startPackageInstallerSessionDetailsActivity(session,null,null));
                    return 102;
                }
            }
            host.unarchivePackage(name, caller, listener(user,name), user, confirmation, Binder.getCallingUid(), Binder.getCallingPid());
        } catch (Throwable failure) {
            android.util.Slog.e("PackageArchiverService", "Unexpected error occurred while unarchiving package " + name + ": " + failure.getLocalizedMessage());
        }
        return 102;
    }
    private boolean qualified(String caller, int uid, int user) {
        if (uid == 2000) return true;
        try (var computer = Objects.requireNonNull(computers.get())) {
            var parent = computer.getProfileParent(user); int parentUser = parent == null ? user : parent.id;
            var home = computer.getDefaultHomeActivity(parentUser);
            if (home != null && caller.equals(home.getPackageName())) return true;
            var state = computer.getPackageStateInternal(caller);
            if (state == null || !state.isSystem()) return false;
            Intent homeIntent = computer.getHomeIntent().setPackage(caller);
            return !computer.queryIntentActivitiesInternal(homeIntent, null, 0, user).isEmpty();
        }
    }
    private synchronized IntentSender listener(int user, String name) {
        var key = new android.util.Pair<Integer,String>(user, name);
        return listeners.computeIfAbsent(key, unused -> new IntentSender((android.content.IIntentSender) new ArchiveListener()));
    }
    private final class ArchiveListener extends android.content.IIntentSender.Stub {
        @Override public void send(int code, Intent intent, String type, android.os.IBinder whitelist,
                android.content.IIntentReceiver finished, String permission, android.os.Bundle options) {
            if (intent.getIntExtra("android.content.pm.extra.UNARCHIVE_STATUS", -1) == 0) return;
            Intent action = intent.getParcelableExtra("android.intent.extra.INTENT", Intent.class);
            UserHandle user = intent.getParcelableExtra("android.intent.extra.USER", UserHandle.class);
            if (action == null || user == null) return;
            try (var computer = Objects.requireNonNull(computers.get())) {
                var home = computer.getDefaultHomeActivity(user.getIdentifier());
                if (home != null && new com.android.server.pm.AppStateHelper(context).isAppTopVisible(home.getPackageName()))
                    context.startActivityAsUser(action.setFlags(0x10000000), user);
            }
        }
    }
    @Override void notifyUnarchivalListener(int status, String installer, String name, long bytes,
            android.app.PendingIntent action, Set<IntentSender> senders, int user) {
        Intent result = new Intent().putExtra("android.content.pm.extra.PACKAGE_NAME", name)
                .putExtra("android.content.pm.extra.UNARCHIVE_STATUS", status);
        if (status != 0) {
            String title;
            try (var scope = packages.computer()) {
                var state = scope.getPackageStateInternal(name);
                var archive = state == null ? null : state.getUserStateOrDefault(user).getArchiveState();
                if (archive == null) return;
                title = archive.getInstallerTitle();
            }
            Intent dialog = new Intent("com.android.intent.action.UNARCHIVE_ERROR_DIALOG")
                    .putExtra("android.intent.extra.USER", UserHandle.of(user))
                    .putExtra("android.content.pm.extra.UNARCHIVE_STATUS", status)
                    .putExtra("com.android.content.pm.extra.UNARCHIVE_INSTALLER_PACKAGE_NAME", installer)
                    .putExtra("com.android.content.pm.extra.UNARCHIVE_INSTALLER_TITLE", title);
            if (bytes > 0) dialog.putExtra("com.android.content.pm.extra.UNARCHIVE_EXTRA_REQUIRED_BYTES", bytes);
            if (action != null) dialog.putExtra("android.intent.extra.INTENT", action);
            result.putExtra("android.intent.extra.INTENT", dialog).putExtra("android.intent.extra.USER", UserHandle.of(user));
        }
        var options = android.app.BroadcastOptions.makeBasic(); options.setPendingIntentBackgroundActivityStartMode(2);
        for (IntentSender sender : senders) {
            try { sender.sendIntent(context, 0, result, null, options.toBundle(), null, null); }
            catch (IntentSender.SendIntentException failure) { android.util.Slog.e("PackageArchiverService", "Failed to send unarchive intent", failure); }
            finally { synchronized (this) { listeners.remove(new android.util.Pair<Integer,String>(user, name)); } }
        }
    }
}
