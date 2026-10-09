package dev.aim.server;

import android.app.ActivityManager;
import android.app.BroadcastOptions;
import android.app.job.JobInfo;
import android.app.job.JobScheduler;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.IntentSender;
import android.content.pm.PackageInstaller;
import android.content.pm.PackageManager;
import android.content.res.Resources;
import android.os.Binder;
import android.os.Bundle;
import android.os.IRemoteCallback;
import android.os.RemoteException;
import android.provider.DeviceConfig;
import com.android.server.pm.AppStateHelper;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;

/** Original service owners behind the native installer's asynchronous effects. */
public final class InstallerExternalBridge extends IInstallerExternalBridge.Stub implements AutoCloseable {
    public interface ExternalSources { boolean trusted(String packageName, int uid); }
    private static volatile ExternalSources externalSources;
    /** Called by the actual native PackageManagerInternal facade's policy setter. */
    public static void setExternalSourcesPolicy(ExternalSources policy) { externalSources = policy; }
    public static boolean isExternalSourceDisabled(String packageName,int uid){ExternalSources source=externalSources;return source!=null&&!source.trusted(packageName,uid);}
    private static final int JOB_ID = 235306967;
    private final Context context;
    private final AppStateHelper appState;
    private final InstallerRemovalBridge removal;
    private final List<ActivityManager.OnUidImportanceListener> listeners = new ArrayList<>();
    private IRemoteCallback idleCallback;
    private record Presentation(PackageInstaller.PreapprovalDetails details, IntentSender receiver) {}
    private final java.util.Map<Integer, Presentation> preapprovals = new java.util.HashMap<>();
    private boolean closed;
    private static final java.util.concurrent.atomic.AtomicReference<InstallerExternalBridge> current = new java.util.concurrent.atomic.AtomicReference<>();

    public InstallerExternalBridge(Context context) {
        this.context = Objects.requireNonNull(context);
        appState = new AppStateHelper(context);
        removal = new InstallerRemovalBridge(context);
    }
    private ActivityManager activities() { return Objects.requireNonNull(context.getSystemService(ActivityManager.class), "ActivityManager owner unavailable"); }
    private JobScheduler jobs() { return Objects.requireNonNull(context.getSystemService(JobScheduler.class), "JobScheduler owner unavailable"); }
    @Override public android.os.IBinder getRemovalBridge() { enforceSystem(); checkOpen(); return removal; }
    /** Bind only at C, with the original GentleUpdate job's entry redirected to onIdleJob. */
    public synchronized void attachIdleJobOwner() {
        checkOpen();
        if (current.get() != this && !current.compareAndSet(null, this)) throw new IllegalStateException("foreign installer idle job owner");
    }
    private static void enforceSystem() {
        int uid = Binder.getCallingUid();
        if (uid != 1000 && uid != 0) throw new SecurityException("untrusted installer bridge caller");
    }
    private synchronized void checkOpen() {
        if (closed) throw new IllegalStateException("installer external owner closed");
    }
    @Override public int getConstraintAppState(List<String> packages, int flags, boolean deviceIdle) {
        enforceSystem(); checkOpen();
        List<String> dependencies = appState.getDependencyPackages(Objects.requireNonNull(packages));
        if ((flags & 1) != 0 && !deviceIdle) return 0;
        if ((flags & 2) != 0 && appState.hasForegroundApp(dependencies)) return 1;
        if ((flags & 4) != 0 && appState.hasInteractingApp(dependencies)) return 2;
        if ((flags & 8) != 0 && appState.hasTopVisibleApp(dependencies)) return 4;
        return (flags & 16) != 0 && appState.isInCall() ? 8 : 0;
    }
    @Override public List<String> getConstraintDependencyPackages(List<String> packages) {
        enforceSystem(); checkOpen();
        return appState.getDependencyPackages(Objects.requireNonNull(packages));
    }
    @Override public synchronized void watchAppState(IRemoteCallback callback) {
        enforceSystem(); checkOpen(); Objects.requireNonNull(callback);
        for (int cut : new int[] {100, 125}) {
            ActivityManager.OnUidImportanceListener listener = (uid, importance) -> {
                try {
                    Bundle event = new Bundle();
                    event.putString("package", android.app.ActivityThread.getPackageManager().getNameForUid(uid));
                    callback.sendResult(event);
                }
                catch (RemoteException failure) { android.util.Slog.e("NativeInstaller", "constraint state callback failed", failure); }
            };
            activities().addOnUidImportanceListener(listener, cut);
            listeners.add(listener);
        }
    }
    @Override public synchronized void requestIdleJob(IRemoteCallback callback) {
        enforceSystem(); checkOpen();
        if (current.get() != this) throw new IllegalStateException("original idle job entry is not attached");
        idleCallback = Objects.requireNonNull(callback);
        if (android.os.SystemProperties.getBoolean("debug.pm.gentle_update_test.is_idle", false)) {
            new android.os.Handler(android.os.Looper.getMainLooper()).post(() -> {
                if (current.get() == this) onIdleJob();
            });
            return;
        }
        JobInfo info = new JobInfo.Builder(JOB_ID, new ComponentName(context.getPackageName(),
                "com.android.server.pm.GentleUpdateHelper$Service"))
                .setRequiresDeviceIdle(true).build();
        if (jobs().schedule(info) != 1) throw new IllegalStateException("installer idle job scheduling failed");
    }
    /** Target of the minimal C-only original JobService entry edit. */
    public static void onIdleJob() {
        InstallerExternalBridge owner = current.get();
        if (owner == null) {
            android.util.Slog.w("NativeInstaller", "idle job arrived after native owner detached");
            return;
        }
        IRemoteCallback callback;
        synchronized (owner) {
            if (owner.closed) {
                android.util.Slog.w("NativeInstaller", "idle job arrived after native owner closed");
                return;
            }
            callback = owner.idleCallback; owner.idleCallback = null;
        }
        if (callback != null) {
            try { callback.sendResult(new Bundle()); }
            catch (RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        }
    }
    @Override public boolean isIntentSenderImmutable(IntentSender receiver) {
        enforceSystem(); checkOpen(); return Objects.requireNonNull(receiver).isImmutable();
    }
    @Override public boolean isCommitMutableReceiverEnforced(int uid) {
        enforceSystem(); checkOpen();
        return android.app.compat.CompatChanges.isChangeEnabled(240618202L, uid);
    }
    @Override public boolean isSecureFrpActive() {
        enforceSystem(); checkOpen();
        if (com.android.internal.hidden_from_bootclasspath.android.security.Flags.frpEnforcement()) {
            android.service.persistentdata.PersistentDataBlockManager owner =
                    context.getSystemService(android.service.persistentdata.PersistentDataBlockManager.class);
            return owner != null && owner.isFactoryResetProtectionActive();
        }
        return android.provider.Settings.Global.getInt(context.getContentResolver(), "secure_frp_mode", 0) == 1;
    }
    @Override public int getUserActionPolicy(String installer, int installerUid, int userId) {
        enforceSystem(); checkOpen(); int flags = 0;
        android.app.admin.DevicePolicyManagerInternal policy = com.android.server.LocalServices.getService(android.app.admin.DevicePolicyManagerInternal.class);
        if (userId == android.os.UserHandle.getUserId(installerUid) && policy != null
                && policy.canSilentlyInstallPackage(installer, installerUid)) flags |= 1;
        long identity = Binder.clearCallingIdentity();
        try {
            if (DeviceConfig.getBoolean("package_manager_service", "is_update_ownership_enforcement_available", true)) flags |= 2;
        } finally { Binder.restoreCallingIdentity(identity); }
        if (com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.sdkDependencyInstaller()) flags |= 4;
        com.android.server.pm.UserManagerService users = com.android.server.pm.UserManagerService.getInstance();
        if (users.hasUserRestriction("no_install_unknown_sources", userId)
                || users.hasUserRestriction("no_install_unknown_sources_globally", userId)) flags |= 8;
        else {
            ExternalSources sources = externalSources;
            if (sources != null && !sources.trusted(installer, installerUid)) flags |= 8;
        }
        return flags;
    }
    @Override public boolean isSilentInstallTargetAllowed(String packageName, int targetSdk) {
        enforceSystem(); checkOpen();
        if (targetSdk == Integer.MAX_VALUE) return false;
        android.content.pm.ApplicationInfo info = new android.content.pm.ApplicationInfo();
        info.packageName = packageName; info.targetSdkVersion = targetSdk;
        com.android.internal.compat.IPlatformCompat compat = com.android.internal.compat.IPlatformCompat.Stub.asInterface(
                android.os.ServiceManager.getService("platform_compat"));
        try { return compat.isChangeEnabled(325888262L, info); }
        catch (RemoteException failure) {
            android.util.Slog.e("NativeInstaller", "silent install compatibility query failed", failure);
            return false;
        }
    }
    @Override public boolean isPreapprovalRequestAvailable() {
        enforceSystem(); checkOpen(); long identity = Binder.clearCallingIdentity();
        try {
            return Resources.getSystem().getBoolean(com.android.internal.R.bool.config_isPreApprovalRequestAvailable)
                && DeviceConfig.getBoolean("package_manager_service", "is_preapproval_available", true);
        } finally { Binder.restoreCallingIdentity(identity); }
    }
    @Override public long getAppMetadataSizeLimit() {
        enforceSystem(); checkOpen(); long identity = Binder.clearCallingIdentity();
        try { return DeviceConfig.getLong("package_manager_service", "app_metadata_byte_size_limit", 32000); }
        finally { Binder.restoreCallingIdentity(identity); }
    }
    @Override public byte[] normalizeInstallerIcon(android.graphics.Bitmap icon) {
        enforceSystem(); checkOpen();
        android.os.Parcel result = android.os.Parcel.obtain();
        android.os.Parcel bitmap = android.os.Parcel.obtain();
        try {
            if (icon == null) { result.writeByteArray(null); result.writeByteArray(null); }
            else {
                int size = activities().getLauncherLargeIconSize();
                if (icon.getWidth() > size * 2 || icon.getHeight() > size * 2)
                    icon = android.graphics.Bitmap.createScaledBitmap(icon, size, size, true);
                boolean previous = bitmap.pushAllowFds(false);
                try { bitmap.writeParcelable(icon, 0); }
                finally { bitmap.restoreAllowFds(previous); }
                java.io.ByteArrayOutputStream png = new java.io.ByteArrayOutputStream();
                if (!icon.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, png))
                    throw new IllegalStateException("installer icon PNG compression failed");
                result.writeByteArray(bitmap.marshall()); result.writeByteArray(png.toByteArray());
            }
            return result.marshall();
        } finally { bitmap.recycle(); result.recycle(); }
    }
    @Override public synchronized String preparePreapproval(int sessionId,
            PackageInstaller.PreapprovalDetails details, IntentSender receiver) {
        enforceSystem(); checkOpen();
        Objects.requireNonNull(details); Objects.requireNonNull(receiver);
        preapprovals.put(sessionId, new Presentation(details, receiver));
        return details.getPackageName();
    }
    @Override public void sendPreapprovalStatus(int sessionId, int legacyStatus,
            String message, String pendingInstallerPackage, boolean preapprovalExtra) {
        enforceSystem(); checkOpen(); Presentation value;
        synchronized (this) { value = preapprovals.get(sessionId); }
        if (value == null) throw new IllegalStateException("preapproval presentation unavailable");
        if (legacyStatus == 1 && pendingInstallerPackage == null) {
            Intent fill = new Intent(); fill.putExtra("android.content.pm.extra.SESSION_ID", sessionId);
            fill.putExtra("android.content.pm.extra.STATUS", 0);
            fill.putExtra("android.content.pm.extra.PRE_APPROVAL", true);
            send(value.receiver(), fill);
        } else {
            sendSessionStatus(value.receiver(), sessionId, value.details().getPackageName(),
                    legacyStatus, message, preapprovalExtra, pendingInstallerPackage);
        }
    }
    @Override public synchronized void clearPreapproval(int sessionId) {
        enforceSystem(); checkOpen(); preapprovals.remove(sessionId);
    }
    private void send(IntentSender receiver, Intent intent) {
        BroadcastOptions options = BroadcastOptions.makeBasic();
        options.setPendingIntentBackgroundActivityLaunchAllowed(false);
        try { receiver.sendIntent(context, 0, intent, null, options.toBundle(), null, null); }
        catch (IntentSender.SendIntentException failure) { throw new IllegalStateException("installer status delivery failed", failure); }
    }
    @Override public void sendPendingStreaming(IntentSender receiver,int id,String cause){
        enforceSystem();checkOpen();Intent event=new Intent();
        event.putExtra("android.content.pm.extra.SESSION_ID",id);event.putExtra("android.content.pm.extra.STATUS",-2);
        event.putExtra("android.content.pm.extra.STATUS_MESSAGE",cause==null||cause.isEmpty()?"Staging Image Not Ready":"Staging Image Not Ready ["+cause+"]");
        send(Objects.requireNonNull(receiver),event);
    }
    @Override public void sendSessionStatus(IntentSender receiver, int sessionId, String packageName,
            int legacyStatus, String message, boolean preapproval, String pendingInstallerPackage) {
        sendSessionStatusWithWarnings(receiver,sessionId,packageName,legacyStatus,message,preapproval,pendingInstallerPackage,new String[0]);
    }
    @Override public void sendSessionStatusWithWarnings(IntentSender receiver, int sessionId, String packageName,
            int legacyStatus, String message, boolean preapproval, String pendingInstallerPackage, String[] warnings) {
        enforceSystem(); checkOpen(); Objects.requireNonNull(receiver);Objects.requireNonNull(warnings);
        Intent fill = new Intent();
        if(warnings.length!=0) fill.putStringArrayListExtra(android.content.pm.PackageInstaller.EXTRA_WARNINGS,new java.util.ArrayList<>(java.util.Arrays.asList(warnings)));
        fill.putExtra("android.content.pm.extra.SESSION_ID", sessionId);
        fill.putExtra("android.content.pm.extra.PRE_APPROVAL", preapproval);
        if (pendingInstallerPackage != null) {
            Intent action = new Intent(preapproval ? "android.content.pm.action.CONFIRM_PRE_APPROVAL" : "android.content.pm.action.CONFIRM_INSTALL");
            action.setPackage(pendingInstallerPackage);
            action.putExtra("android.content.pm.extra.SESSION_ID", sessionId);
            fill.putExtra("android.content.pm.extra.STATUS", -1);
            fill.putExtra("android.intent.extra.INTENT", action);
        } else {
            fill.putExtra("android.content.pm.extra.PACKAGE_NAME", packageName);
            fill.putExtra("android.content.pm.extra.STATUS", PackageManager.installStatusToPublicStatus(legacyStatus));
            fill.putExtra("android.content.pm.extra.STATUS_MESSAGE", PackageManager.installStatusToString(legacyStatus, message));
            fill.putExtra("android.content.pm.extra.LEGACY_STATUS", legacyStatus);
        }
        send(receiver, fill);
    }
    @Override public void sendConstraintCallback(IRemoteCallback callback, boolean satisfied) throws RemoteException {
        enforceSystem(); checkOpen(); Bundle result = new Bundle();
        result.putParcelable("result", new PackageInstaller.InstallConstraintsResult(satisfied));
        Objects.requireNonNull(callback).sendResult(result);
    }
    @Override public void sendConstraintIntent(IntentSender callback, List<String> packages, int flags, boolean satisfied) {
        enforceSystem(); checkOpen();
        Intent fill = new Intent(); fill.putExtra(Intent.EXTRA_PACKAGES, packages.toArray(new String[0]));
        fill.putExtra("android.content.pm.extra.INSTALL_CONSTRAINTS", new PackageInstaller.InstallConstraints(
                (flags & 1) != 0, (flags & 2) != 0, (flags & 4) != 0, (flags & 8) != 0, (flags & 16) != 0));
        fill.putExtra("android.content.pm.extra.INSTALL_CONSTRAINTS_RESULT", new PackageInstaller.InstallConstraintsResult(satisfied));
        send(Objects.requireNonNull(callback), fill);
    }
    @Override public synchronized void close() {
        enforceSystem();
        if (closed) return; closed = true;
        for (ActivityManager.OnUidImportanceListener listener : listeners) activities().removeOnUidImportanceListener(listener);
        listeners.clear(); idleCallback = null; preapprovals.clear();
        if (current.compareAndSet(this, null)) jobs().cancel(JOB_ID);
    }
    @Override public byte[] getNativeInstallEnvironment() {
        enforceSystem(); checkOpen();
        android.os.Parcel parcel = android.os.Parcel.obtain();
        try {
            parcel.writeLong(android.system.Os.sysconf(android.system.OsConstants._SC_PAGESIZE));
            parcel.writeBoolean(android.os.SystemProperties.getBoolean("pm.16kb.app_compat.disabled", false));
            parcel.writeBoolean(android.os.UserManager.isHeadlessSystemUserMode());
            parcel.writeString(java.util.TimeZone.getDefault().getID());
            return parcel.marshall();
        } finally { parcel.recycle(); }
    }
    @Override public int getZipLocalUtcOffset(int year, int month, int day, int hour, int minute, int second) {
        enforceSystem(); checkOpen();
        java.util.TimeZone zone = java.util.TimeZone.getDefault();
        java.util.GregorianCalendar calendar = new java.util.GregorianCalendar(zone);
        calendar.clear(); calendar.setLenient(true);
        calendar.set(year, month - 1, day, hour, minute, second);
        return Math.toIntExact(zone.getOffset(calendar.getTimeInMillis()) / 1000L);
    }
}
