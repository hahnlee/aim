package dev.darwinart.runtime.am;

import android.os.Binder;
import android.os.Debug;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.content.Intent;
import android.util.Log;
import dev.darwinart.runtime.content.SettingsProviderEndpoint;
import dev.darwinart.runtime.pm.PackageRecords;
import java.lang.reflect.Field;

/** System-process application attachment owner. Unsupported AMS calls stay unsupported. */
public final class ActivityManagerEndpoint extends Binder {
    private final int attachCode = transaction("attachApplication");
    private final int finishCode = transaction("finishAttachApplication");
    private final int getContentProviderCode = transaction("getContentProvider");
    private final int bindServiceInstanceCode = transaction("bindServiceInstance");
    private final int unbindServiceCode = transaction("unbindService");
    private final int publishServiceCode = transaction("publishService");
    private final int serviceDoneExecutingCode = transaction("serviceDoneExecuting");
    private final int unbindFinishedCode = transaction("unbindFinished");
    private final int getProcessMemoryInfoCode = transaction("getProcessMemoryInfo");
    private final PackageRecords.Source packages;
    private final ApplicationProcessRegistry processes;
    private final SettingsProviderEndpoint settingsProvider = new SettingsProviderEndpoint();
    private final ActiveServices activeServices;
    private final BoundServiceProcessLauncher processLauncher;

    public ActivityManagerEndpoint(
            PackageRecords.Source packages, ApplicationProcessRegistry processes) {
        this.packages = packages;
        this.processes = processes;
        processLauncher = new BoundServiceProcessLauncher(processes);
        activeServices = new ActiveServices(packages, processes, processLauncher);
        attachInterface(null, "android.app.IActivityManager");
    }

    /** Android system-service-only entry; this is not exposed on IActivityManager. */
    public SystemServiceBindings systemServiceBindings() {
        return activeServices;
    }

    private static int transaction(String name) {
        try {
            Field field = Class.forName("android.app.IActivityManager$Stub")
                    .getDeclaredField("TRANSACTION_" + name);
            field.setAccessible(true);
            return field.getInt(null);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static native String nativeResolveAttachedPackage(int expectedUid);
    private static native String nativeAttach(IBinder application, long startSequence,
            String reservedProcessName, int expectedUid);
    private static native void nativeLaunch(IBinder application, String packageName, String record);

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (System.getenv("DARWIN_ART_DEBUG_BINDER") != null) {
            Log.i("DarwinActivityManager", "onTransact code=" + code
                    + " attach=" + attachCode + " finish=" + finishCode
                    + " reply=" + (reply != null));
        }
        if (code != attachCode && code != finishCode && code != getContentProviderCode
                && code != bindServiceInstanceCode && code != unbindServiceCode
                && code != publishServiceCode && code != serviceDoneExecutingCode
                && code != unbindFinishedCode && code != getProcessMemoryInfoCode) {
            return super.onTransact(code, data, reply, flags);
        }
        if (reply == null && code != serviceDoneExecutingCode) return false;
        data.enforceInterface("android.app.IActivityManager");
        if (code == serviceDoneExecutingCode) {
            IBinder token = data.readStrongBinder();
            int type = data.readInt();
            int startId = data.readInt();
            int result = data.readInt();
            Intent intent = data.readTypedObject(Intent.CREATOR);
            data.enforceNoDataAvail();
            activeServices.serviceDoneExecuting(
                    Binder.getCallingPid(), Binder.getCallingUid(),
                    token, type, startId, result, intent);
            return true;
        }
        if (code == getProcessMemoryInfoCode) {
            int[] pids = data.createIntArray();
            data.enforceNoDataAvail();
            if (pids == null) pids = new int[0];
            Debug.MemoryInfo[] result = new Debug.MemoryInfo[pids.length];
            for (int index = 0; index < pids.length; index++) {
                // This bounded AMS slice admits only live processes owned by this system server.
                // Counters remain zero (Android's unavailable value) until the Darwin process
                // metrics provider is connected; preserving the requested array shape prevents
                // clients from mistaking an unsupported metric for a vanished process.
                processes.requireAttachedProcess(pids[index]);
                result[index] = new Debug.MemoryInfo();
            }
            reply.writeNoException();
            reply.writeTypedArray(result, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == bindServiceInstanceCode) {
            IBinder caller = data.readStrongBinder();
            IBinder activityToken = data.readStrongBinder();
            Intent intent = data.readTypedObject(Intent.CREATOR);
            String resolvedType = data.readString();
            IBinder connection = data.readStrongBinder();
            long bindFlags = data.readLong();
            String instanceName = data.readString();
            String callingPackage = data.readString();
            int userId = data.readInt();
            data.enforceNoDataAvail();
            int result = activeServices.bindServiceInstance(Binder.getCallingPid(),
                    Binder.getCallingUid(), caller, activityToken, intent, resolvedType,
                    connection, bindFlags, instanceName, callingPackage, userId);
            reply.writeNoException();
            reply.writeInt(result);
            return true;
        }
        if (code == unbindServiceCode) {
            IBinder connection = data.readStrongBinder();
            data.enforceNoDataAvail();
            boolean result = activeServices.unbindService(connection);
            reply.writeNoException();
            reply.writeBoolean(result);
            return true;
        }
        if (code == publishServiceCode) {
            IBinder token = data.readStrongBinder();
            Intent intent = data.readTypedObject(Intent.CREATOR);
            IBinder published = data.readStrongBinder();
            data.enforceNoDataAvail();
            activeServices.publishService(Binder.getCallingPid(), token, intent, published);
            reply.writeNoException();
            return true;
        }
        if (code == unbindFinishedCode) {
            IBinder token = data.readStrongBinder();
            Intent intent = data.readTypedObject(Intent.CREATOR);
            data.enforceNoDataAvail();
            activeServices.unbindFinished(Binder.getCallingPid(), token, intent);
            reply.writeNoException();
            return true;
        }
        if (code == getContentProviderCode) {
            data.readStrongBinder(); // IApplicationThread caller.
            data.readString(); // Calling package; identity comes from Binder credentials.
            String authority = data.readString();
            int userId = data.readInt();
            data.readBoolean(); // Stable/unstable reference.
            data.enforceNoDataAvail();
            Parcelable holder = "settings".equals(authority) && userId == 0
                    ? settingsProvider.holder() : null;
            reply.writeNoException();
            reply.writeTypedObject(holder, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        int pid = Binder.getCallingPid();
        if (pid <= 0) throw new SecurityException("Unidentified application process");
        if (code == attachCode) {
            IBinder app = data.readStrongBinder();
            long sequence = data.readLong();
            data.enforceNoDataAvail();
            if (app == null || sequence < 0) throw new IllegalArgumentException("Invalid attachment");
            int callerUid = Binder.getCallingUid();
            processes.beginAttachment(pid, callerUid, app, sequence);
            try {
                // Binder credentials and the native process registry are the
                // authority for package identity. Publish that identity before
                // bindApplication: Application.attachBaseContext may call back
                // into AMS/ATMS while the bind transaction is still running.
                String trustedPackage = nativeResolveAttachedPackage(callerUid);
                if (trustedPackage == null || trustedPackage.isEmpty()) {
                    throw new SecurityException("Attachment has no authenticated package");
                }
                processes.identify(pid, sequence, app, trustedPackage);
                ApplicationProcessRegistry.AttachedApplication target =
                        processes.attachmentTarget(pid, sequence);
                // Observe this exact incarnation before entering native bind.
                // A Binder death can race with bindApplication and must not
                // remove a later process that reuses the same PID.
                observeProcessDeath(target);
                // linkToDeath reports an already-dead Binder synchronously.
                // Re-read the keyed record so that case cannot proceed into a
                // native bind after the registry has retired the incarnation.
                target = processes.attachmentTarget(pid, sequence);
                String packageName = nativeAttach(
                        app, sequence, target.processName, target.uid);
                if (packageName == null || !trustedPackage.equals(packageName)
                        || !target.packageName.equals(packageName)) {
                    throw new SecurityException("Native attachment identity does not match");
                }
            } catch (Throwable error) {
                processes.abortAttachment(pid, sequence, app);
                if (error instanceof RemoteException) throw (RemoteException) error;
                if (error instanceof RuntimeException) throw (RuntimeException) error;
                if (error instanceof Error) throw (Error) error;
                throw new IllegalStateException(error);
            }
        } else {
            try {
                long sequence = data.readLong();
                data.readLong(); // Application.onCreate timestamp, not a process identity.
                data.enforceNoDataAvail();
                ApplicationProcessRegistry.AttachedApplication attached =
                        processes.finishAttachment(pid, sequence);
                processLauncher.onAttached(attached);
                IBinder app = attached.thread;
                String packageName = attached.packageName;
                String record = packageName == null
                        ? null : packages.resolveInstalledPackage(packageName);
                if (attached.initialWork == ApplicationProcessRegistry.InitialWork.ACTIVITY) {
                    nativeLaunch(app, packageName, record);
                }
                activeServices.onProcessAttached(attached);
            } catch (RuntimeException | Error error) {
                Log.e("DarwinActivityManager", "finishAttachApplication failed", error);
                throw error;
            }
        }
        reply.writeNoException();
        return true;
    }

    private void observeProcessDeath(ApplicationProcessRegistry.AttachedApplication attached)
            throws RemoteException {
        IBinder.DeathRecipient recipient = () -> {
            processes.retireAttached(attached.pid, attached.startSequence, attached.thread);
            // This obituary already proves death of its captured Binder. A
            // failed attachment may have removed the registry row first; that
            // must not suppress exact launch/client resource cleanup. Both
            // owners reject stale incarnation identities independently.
            activeServices.onProcessGone(attached);
        };
        try {
            attached.thread.linkToDeath(recipient, 0);
        } catch (RemoteException registrationFailure) {
            boolean terminal;
            try { terminal = !attached.thread.isBinderAlive(); }
            catch (RuntimeException | Error queryFailure) {
                if (queryFailure != registrationFailure) registrationFailure.addSuppressed(queryFailure);
                throw registrationFailure;
            }
            if (!terminal) throw registrationFailure;
            recipient.binderDied();
        }
    }
}
