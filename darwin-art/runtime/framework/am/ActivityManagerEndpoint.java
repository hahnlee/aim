package dev.darwinart.runtime.am;

import android.app.ActivityManager;
import android.os.Binder;
import android.os.Debug;
import android.os.IBinder;
import android.os.Parcel;
import android.os.Parcelable;
import android.os.RemoteException;
import android.app.Notification;
import android.content.ComponentName;
import android.content.Intent;
import android.util.Log;
import android.content.pm.ActivityInfo;
import dev.darwinart.runtime.content.SettingsProviderEndpoint;
import java.lang.reflect.Field;

/** System-process application attachment owner. Unsupported AMS calls stay unsupported. */
public final class ActivityManagerEndpoint extends Binder {
    /** ActivityTask owner of each attached process's desktop task geometry. */
    public interface TaskLifecycle {
        void prepareProcess(int pid, IBinder thread, ActivityInfo launchActivity);
        void removeProcess(int pid, IBinder thread);
    }

    private final int attachCode = transaction("attachApplication");
    private final int finishCode = transaction("finishAttachApplication");
    private final int getContentProviderCode = transaction("getContentProvider");
    private final int bindServiceInstanceCode = transaction("bindServiceInstance");
    private final int unbindServiceCode = transaction("unbindService");
    private final int publishServiceCode = transaction("publishService");
    private final int serviceDoneExecutingCode = transaction("serviceDoneExecuting");
    private final int unbindFinishedCode = transaction("unbindFinished");
    private final int getProcessMemoryInfoCode = transaction("getProcessMemoryInfo");
    private final int startServiceCode = transaction("startService");
    private final int stopServiceCode = transaction("stopService");
    private final int stopServiceTokenCode = transaction("stopServiceToken");
    private final int setServiceForegroundCode = transaction("setServiceForeground");
    private final int getForegroundServiceTypeCode = transaction("getForegroundServiceType");
    private final int getRunningAppProcessesCode = transaction("getRunningAppProcesses");
    private final int getMemoryInfoCode = transaction("getMemoryInfo");
    private final int getProcessesInErrorStateCode = transaction("getProcessesInErrorState");
    private final int frozenBinderTransactionDetectedCode =
            transaction("frozenBinderTransactionDetected");
    private final int registerReceiverWithFeatureCode =
            transaction("registerReceiverWithFeature");
    private final int unregisterReceiverCode = transaction("unregisterReceiver");
    private final int broadcastIntentWithFeatureCode = transaction("broadcastIntentWithFeature");
    private final int getInfoForIntentSenderCode = transaction("getInfoForIntentSender");
    private final int sendIntentSenderCode = transaction("sendIntentSender");
    private final int registerUidObserverCode = transaction("registerUidObserver");
    private final int registerUidObserverForUidsCode =
            transaction("registerUidObserverForUids");
    private final int unregisterUidObserverCode = transaction("unregisterUidObserver");
    private final int addUidToObserverCode = transaction("addUidToObserver");
    private final int removeUidFromObserverCode = transaction("removeUidFromObserver");
    private final int handleApplicationWtfCode = transaction("handleApplicationWtf");
    private final int handleApplicationCrashCode = transaction("handleApplicationCrash");
    private final int handleApplicationStrictModeViolationCode =
            transaction("handleApplicationStrictModeViolation");
    private final int setRenderThreadCode = transaction("setRenderThread");
    private final int publishContentProvidersCode = transaction("publishContentProviders");
    private final int handleIncomingUserCode = transaction("handleIncomingUser");
    private final IncomingUsers incomingUsers = new IncomingUsers(ApplicationPackages::hasPermission);
    // ProcessRecord.mRenderThreadTid, by pid.
    private final java.util.concurrent.ConcurrentHashMap<Integer, Integer> renderThreads =
            new java.util.concurrent.ConcurrentHashMap<>();
    // ContentProviderRecords by authority: the holder a process published with
    // the application thread that owns it.
    private final java.util.HashMap<String, PublishedProvider> providers =
            new java.util.HashMap<>();

    private static final class PublishedProvider {
        final IBinder owner;
        final int uid;
        final android.app.ContentProviderHolder holder;

        PublishedProvider(IBinder owner, int uid, android.app.ContentProviderHolder holder) {
            this.owner = owner;
            this.uid = uid;
            this.holder = holder;
        }
    }
    private final BroadcastTransactions broadcasts;
    private final ApplicationProcessRegistry processes;
    private final SettingsProviderEndpoint settingsProvider = new SettingsProviderEndpoint();
    private final ActiveServices activeServices;
    private final BoundServiceProcessLauncher processLauncher;
    private final TaskLifecycle tasks;
    private final UidProcessStates uidStates;
    private final AppErrors appErrors;

    public ActivityManagerEndpoint(ApplicationProcessRegistry processes, TaskLifecycle tasks) {
        if (tasks == null) throw new NullPointerException("tasks");
        this.processes = processes;
        this.tasks = tasks;
        processLauncher = new BoundServiceProcessLauncher(processes);
        activeServices = new ActiveServices(ApplicationPackages.INSTANCE,
                processes, processLauncher);
        broadcasts = new BroadcastTransactions(processes);
        uidStates = new UidProcessStates(processes);
        appErrors = new AppErrors(processes);
        attachInterface(null, "android.app.IActivityManager");
    }

    /** Android system-service-only entry; this is not exposed on IActivityManager. */
    /** ActivityManagerService.setSystemProcess for this process's ActivityThread. */
    public void setSystemProcess(IBinder applicationThread) {
        processes.setSystemProcess(android.os.Process.myPid(), applicationThread);
    }

    /**
     * ActivityManagerService's AppOpsService (created by SystemServer here):
     * uid process state and capability changes are reported to it.
     */
    public void setAppOpsService(com.android.server.appop.AppOpsService appOps) {
        uidStates.attach(appOps);
    }

    public SystemServiceBindings systemServiceBindings() {
        return activeServices;
    }

    /** The ActivityManagerInternal this endpoint's state answers. */
    public android.app.ActivityManagerInternal localService() {
        return new ActivityManagerLocal(broadcasts);
    }

    /** Android system-service-only broadcast entry; not exposed on IActivityManager. */
    public SystemBroadcasts systemBroadcasts() {
        return broadcasts;
    }

    /** ActivityManagerService.publishContentProviders for {@code caller}'s process. */
    private void publishProviders(IBinder caller, int uid,
            java.util.List<android.app.ContentProviderHolder> published) {
        if (caller == null || published == null) return;
        synchronized (providers) {
            for (android.app.ContentProviderHolder holder : published) {
                if (holder == null || holder.info == null || holder.info.authority == null
                        || holder.provider == null) continue;
                if (holder.info.applicationInfo != null
                        && holder.info.applicationInfo.uid != uid) {
                    throw new SecurityException("Provider " + holder.info.authority
                            + " does not belong to the publishing uid " + uid);
                }
                for (String authority : holder.info.authority.split(";")) {
                    providers.put(authority, new PublishedProvider(caller, uid, holder));
                }
            }
        }
    }

    /**
     * A provider another process of the caller's own uid published (a
     * multi-process app). Cross-package access needs AMS's provider
     * permission checks, which are not enforced here yet (#107).
     */
    private android.app.ContentProviderHolder publishedProvider(String authority, int uid) {
        PublishedProvider provider;
        synchronized (providers) {
            provider = authority == null ? null : providers.get(authority);
            if (provider == null) return null;
            boolean alive = false;
            for (ApplicationProcessRegistry.AttachedApplication app : processes.attached()) {
                if (app.thread == provider.owner) {
                    alive = true;
                    break;
                }
            }
            if (!alive) {
                providers.values().removeIf(entry -> entry.owner == provider.owner);
                return null;
            }
        }
        return provider.uid == uid ? provider.holder : null;
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
    private static native void nativeLaunch(IBinder application, String packageName, int uid);

    @Override
    protected boolean onTransact(int code, Parcel data, Parcel reply, int flags)
            throws RemoteException {
        if (System.getenv("DARWIN_ART_DEBUG_BINDER") != null) {
            Log.i("DarwinActivityManager", "onTransact code=" + code
                    + " attach=" + attachCode + " finish=" + finishCode
                    + " reply=" + (reply != null));
        }
        if (code == registerReceiverWithFeatureCode || code == unregisterReceiverCode
                || code == broadcastIntentWithFeatureCode) {
            data.enforceInterface("android.app.IActivityManager");
            if (reply == null) return false;
            if (code == registerReceiverWithFeatureCode) {
                broadcasts.register(data, reply);
            } else if (code == unregisterReceiverCode) {
                broadcasts.unregister(data, reply);
            } else {
                return broadcasts.broadcast(data, reply);
            }
            return true;
        }
        if (code == getInfoForIntentSenderCode) {
            data.enforceInterface("android.app.IActivityManager");
            data.readStrongBinder(); // IIntentSender
            data.enforceNoDataAvail();
            if (reply == null) return false;
            // ActivityManagerService.getInfoForIntentSender. This service
            // issues no PendingIntentRecords, so every sender is one the
            // caller made itself (a local IIntentSender): unknown creator.
            reply.writeNoException();
            reply.writeTypedObject(new ActivityManager.PendingIntentInfo(null,
                    android.os.Process.INVALID_UID, false,
                    ActivityManager.INTENT_SENDER_UNKNOWN),
                    Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == sendIntentSenderCode) {
            data.enforceInterface("android.app.IActivityManager");
            data.readStrongBinder(); // IApplicationThread caller
            android.content.IIntentSender target =
                    android.content.IIntentSender.Stub.asInterface(data.readStrongBinder());
            IBinder allowlistToken = data.readStrongBinder();
            int resultCode = data.readInt();
            Intent intent = data.readTypedObject(Intent.CREATOR);
            String resolvedType = data.readString();
            android.content.IIntentReceiver finishedReceiver =
                    android.content.IIntentReceiver.Stub.asInterface(data.readStrongBinder());
            String requiredPermission = data.readString();
            android.os.Bundle options = data.readTypedObject(android.os.Bundle.CREATOR);
            data.enforceNoDataAvail();
            if (reply == null) return false;
            reply.writeNoException();
            reply.writeInt(sendIntentSender(target, allowlistToken, resultCode, intent,
                    resolvedType, finishedReceiver, requiredPermission, options));
            return true;
        }
        if (code == registerUidObserverCode || code == registerUidObserverForUidsCode) {
            data.enforceInterface("android.app.IActivityManager");
            android.app.IUidObserver observer =
                    android.app.IUidObserver.Stub.asInterface(data.readStrongBinder());
            int which = data.readInt();
            int cutpoint = data.readInt();
            data.readString(); // Calling package.
            int[] uids = code == registerUidObserverForUidsCode ? data.createIntArray() : null;
            data.enforceNoDataAvail();
            IBinder token = uidStates.observers().register(observer, which, cutpoint,
                    code == registerUidObserverForUidsCode && uids == null ? new int[0] : uids);
            if (reply != null) {
                reply.writeNoException();
                if (code == registerUidObserverForUidsCode) reply.writeStrongBinder(token);
            }
            return true;
        }
        if (code == unregisterUidObserverCode) {
            data.enforceInterface("android.app.IActivityManager");
            android.app.IUidObserver observer =
                    android.app.IUidObserver.Stub.asInterface(data.readStrongBinder());
            data.enforceNoDataAvail();
            uidStates.observers().unregister(observer);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == addUidToObserverCode || code == removeUidFromObserverCode) {
            data.enforceInterface("android.app.IActivityManager");
            IBinder token = data.readStrongBinder();
            data.readString(); // Calling package.
            int uid = data.readInt();
            data.enforceNoDataAvail();
            uidStates.observers().updateFilter(token, uid, code == addUidToObserverCode);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == handleApplicationCrashCode) {
            data.enforceInterface("android.app.IActivityManager");
            data.readStrongBinder(); // Application thread.
            android.app.ApplicationErrorReport.ParcelableCrashInfo crash = data.readTypedObject(
                    android.app.ApplicationErrorReport.ParcelableCrashInfo.CREATOR);
            data.enforceNoDataAvail();
            int pid = Binder.getCallingPid();
            appErrors.crashApplication(pid, Binder.getCallingUid(),
                    processes.requireIdentifiedProcess(pid), crash);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == handleApplicationStrictModeViolationCode) {
            data.enforceInterface("android.app.IActivityManager");
            data.readStrongBinder(); // Application thread.
            int penalty = data.readInt();
            android.os.StrictMode.ViolationInfo violation =
                    data.readTypedObject(android.os.StrictMode.ViolationInfo.CREATOR);
            data.enforceNoDataAvail();
            // StrictMode penaltyDropBox: AMS records the violation (DropBox).
            Log.w("DarwinActivityManager", "StrictMode violation pid="
                    + Binder.getCallingPid() + " penalty=0x" + Integer.toHexString(penalty)
                    + (violation == null ? "" : " " + violation.getStackTrace()));
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == handleApplicationWtfCode) {
            data.enforceInterface("android.app.IActivityManager");
            data.readStrongBinder(); // Application thread.
            String tag = data.readString();
            boolean system = data.readBoolean();
            android.app.ApplicationErrorReport.ParcelableCrashInfo crash = data.readTypedObject(
                    android.app.ApplicationErrorReport.ParcelableCrashInfo.CREATOR);
            data.readInt(); // Immediate caller pid.
            data.enforceNoDataAvail();
            // ActivityManagerService.handleApplicationWtf: record the report;
            // a WTF ends the process only when Settings.Global.WTF_IS_FATAL,
            // which this runtime leaves off.
            Log.e("DarwinActivityManager", "WTF pid=" + Binder.getCallingPid()
                    + " system=" + system + " tag=" + tag
                    + (crash == null ? "" : " " + crash.exceptionClassName + ": "
                            + crash.exceptionMessage + " at " + crash.throwFileName + ":"
                            + crash.throwLineNumber));
            if (reply != null) {
                reply.writeNoException();
                reply.writeBoolean(false);
            }
            return true;
        }
        if (code == setRenderThreadCode) {
            data.enforceInterface("android.app.IActivityManager");
            int tid = data.readInt();
            data.enforceNoDataAvail();
            // ActivityManagerService.setRenderThread records the tid for the
            // top-app scheduling boost; there is no such scheduling policy here.
            if (tid > 0) renderThreads.put(Binder.getCallingPid(), tid);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == handleIncomingUserCode) {
            data.enforceInterface("android.app.IActivityManager");
            data.readInt();  // callingPid
            int callingUid = data.readInt();
            int userId = data.readInt();
            boolean allowAll = data.readBoolean();
            boolean requireFull = data.readBoolean();
            String name = data.readString();
            data.readString();  // callerPackage
            data.enforceNoDataAvail();
            if (reply == null) return false;
            // As in ActivityManagerService, the result only names a user; it
            // grants nothing, so the supplied uid is evaluated as given.
            int result = incomingUsers.handle(callingUid, userId, allowAll, requireFull, name);
            reply.writeNoException();
            reply.writeInt(result);
            return true;
        }
        if (code == publishContentProvidersCode) {
            data.enforceInterface("android.app.IActivityManager");
            IBinder caller = data.readStrongBinder();
            java.util.ArrayList<android.app.ContentProviderHolder> published =
                    data.createTypedArrayList(android.app.ContentProviderHolder.CREATOR);
            data.enforceNoDataAvail();
            int uid = Binder.getCallingUid();
            publishProviders(caller, uid, published);
            if (reply != null) reply.writeNoException();
            return true;
        }
        if (code == getMemoryInfoCode || code == getProcessesInErrorStateCode
                || code == frozenBinderTransactionDetectedCode) {
            data.enforceInterface("android.app.IActivityManager");
            if (code == frozenBinderTransactionDetectedCode) {
                ProcessStateQueries.frozenBinderTransactionDetected(data);
                return true;
            }
            data.enforceNoDataAvail();
            if (reply == null) return false;
            if (code == getMemoryInfoCode) {
                ProcessStateQueries.writeMemoryInfo(reply);
            } else {
                reply.writeNoException();
                reply.writeTypedList(appErrors.errorStates(Binder.getCallingUid()),
                        Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            }
            return true;
        }
        if (code != attachCode && code != finishCode && code != getContentProviderCode
                && code != bindServiceInstanceCode && code != unbindServiceCode
                && code != publishServiceCode && code != serviceDoneExecutingCode
                && code != unbindFinishedCode && code != getProcessMemoryInfoCode
                && code != startServiceCode && code != stopServiceCode
                && code != stopServiceTokenCode && code != setServiceForegroundCode
                && code != getForegroundServiceTypeCode
                && code != getRunningAppProcessesCode) {
            return dev.darwinart.runtime.os.UnsupportedTransactions.reject(this, code, reply, flags)
                || super.onTransact(code, data, reply, flags);
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
        if (code == getRunningAppProcessesCode) {
            data.enforceNoDataAvail();
            reply.writeNoException();
            reply.writeTypedList(RunningAppProcesses.forCaller(processes, Binder.getCallingUid()),
                    Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == startServiceCode) {
            IBinder caller = data.readStrongBinder();
            Intent intent = data.readTypedObject(Intent.CREATOR);
            data.readString(); // resolvedType; explicit components only.
            data.readBoolean(); // requireForeground: startForeground is tracked when called.
            String callingPackage = data.readString();
            data.readString(); // callingFeatureId
            int userId = data.readInt();
            data.enforceNoDataAvail();
            ComponentName started = activeServices.startService(Binder.getCallingPid(),
                    Binder.getCallingUid(), caller, intent, callingPackage, userId);
            reply.writeNoException();
            reply.writeTypedObject(started, Parcelable.PARCELABLE_WRITE_RETURN_VALUE);
            return true;
        }
        if (code == stopServiceCode) {
            IBinder caller = data.readStrongBinder();
            Intent intent = data.readTypedObject(Intent.CREATOR);
            data.readString(); // resolvedType
            int userId = data.readInt();
            data.enforceNoDataAvail();
            int stopped = activeServices.stopService(Binder.getCallingPid(),
                    Binder.getCallingUid(), caller, intent, userId);
            reply.writeNoException();
            reply.writeInt(stopped);
            return true;
        }
        if (code == stopServiceTokenCode) {
            ComponentName className = data.readTypedObject(ComponentName.CREATOR);
            IBinder token = data.readStrongBinder();
            int startId = data.readInt();
            data.enforceNoDataAvail();
            boolean stopped = activeServices.stopServiceToken(
                    Binder.getCallingPid(), className, token, startId);
            reply.writeNoException();
            reply.writeBoolean(stopped);
            return true;
        }
        if (code == setServiceForegroundCode) {
            ComponentName className = data.readTypedObject(ComponentName.CREATOR);
            IBinder token = data.readStrongBinder();
            int id = data.readInt();
            Notification notification = data.readTypedObject(Notification.CREATOR);
            int foregroundFlags = data.readInt();
            int foregroundServiceType = data.readInt();
            data.enforceNoDataAvail();
            activeServices.setServiceForeground(Binder.getCallingPid(), className, token, id,
                    notification != null, foregroundFlags, foregroundServiceType);
            reply.writeNoException();
            return true;
        }
        if (code == getForegroundServiceTypeCode) {
            ComponentName className = data.readTypedObject(ComponentName.CREATOR);
            IBinder token = data.readStrongBinder();
            data.enforceNoDataAvail();
            int type = activeServices.getForegroundServiceType(
                    Binder.getCallingPid(), className, token);
            reply.writeNoException();
            reply.writeInt(type);
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
                    ? settingsProvider.holder()
                    : publishedProvider(authority, Binder.getCallingUid());
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
                // bindApplication must already carry the launch Activity's
                // task orientation, so resolve the task before binding.
                ActivityInfo launchActivity = target.initialWork
                        == ApplicationProcessRegistry.InitialWork.ACTIVITY
                        ? ApplicationPackages.launchActivity(trustedPackage, callerUid)
                        : null;
                tasks.prepareProcess(pid, app, launchActivity);
                String packageName = nativeAttach(
                        app, sequence, target.processName, target.uid);
                if (packageName == null || !trustedPackage.equals(packageName)
                        || !target.packageName.equals(packageName)) {
                    throw new SecurityException("Native attachment identity does not match");
                }
            } catch (Throwable error) {
                processes.abortAttachment(pid, sequence, app);
                tasks.removeProcess(pid, app);
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
                if (attached.initialWork == ApplicationProcessRegistry.InitialWork.ACTIVITY) {
                    nativeLaunch(app, packageName, attached.uid);
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

    /**
     * ActivityManagerService.sendIntentSender for a sender that is not a
     * PendingIntentRecord, the only kind this service sees (it issues none):
     * the sender is called directly and the finish is reported at once.
     * Intent creator tokens are not issued by this runtime.
     */
    private static int sendIntentSender(android.content.IIntentSender target,
            IBinder allowlistToken, int code, Intent intent, String resolvedType,
            android.content.IIntentReceiver finishedReceiver, String requiredPermission,
            android.os.Bundle options) {
        if (target == null) throw new IllegalArgumentException("Null IIntentSender");
        if (intent == null) {
            Log.wtf("DarwinActivityManager", "Can't use null intent with direct IIntentSender call");
            intent = new Intent(Intent.ACTION_MAIN);
        }
        if (allowlistToken != null) {
            Log.wtf("DarwinActivityManager", "Send a non-null allowlistToken to a non-PI target;"
                    + " intent: " + intent);
        }
        try {
            target.send(code, intent, resolvedType, null, null, requiredPermission, options);
        } catch (RemoteException ignored) {
        }
        if (finishedReceiver != null) {
            try {
                finishedReceiver.performReceive(intent, 0, null, null, false, false,
                        android.os.UserHandle.getCallingUserId());
            } catch (RemoteException ignored) {
            }
        }
        return 0;
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
            broadcasts.registry.processGone(attached.pid);
            tasks.removeProcess(attached.pid, attached.thread);
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
