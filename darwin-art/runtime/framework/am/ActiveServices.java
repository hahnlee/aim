package dev.darwinart.runtime.am;

import android.app.IApplicationThread;
import android.app.IServiceConnection;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.content.res.CompatibilityInfo;
import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteException;
import dev.darwinart.runtime.pm.InstalledServiceInfo;
import dev.darwinart.runtime.pm.PackageRecords;
import java.util.ArrayList;
import java.util.HashMap;

/** Android service lifecycle owner for system_server's ActivityManager endpoint. */
public final class ActiveServices implements SystemServiceBindings {
    interface ProcessLauncher {
        int start(String packageName, String processName, int uid, boolean isolated,
                long startSequence);
    }

    private static final int FIRST_ISOLATED_UID = 99000;
    private static final int LAST_ISOLATED_UID = 99999;
    // Android 16 ActivityManager.PROCESS_STATE_SERVICE. This value is part of
    // the version-pinned IApplicationThread contract and is inlined here
    // because the public SDK compile jar omits the hidden constant.
    private static final int PROCESS_STATE_SERVICE = 10;

    private final PackageRecords.Source packages;
    private final ApplicationProcessRegistry processes;
    private final ProcessLauncher launcher;
    private final HashMap<String, ServiceRecord> services = new HashMap<>();
    private final HashMap<IBinder, ServiceRecord> servicesByToken = new HashMap<>();
    private final HashMap<IBinder, ArrayList<ConnectionRecord>> connections = new HashMap<>();
    private final HashMap<IBinder, IBinder.DeathRecipient> connectionDeaths = new HashMap<>();
    private long nextSequence = 1;
    private int nextIsolatedUid = FIRST_ISOLATED_UID;

    ActiveServices(PackageRecords.Source packageSource, ApplicationProcessRegistry processRegistry,
            ProcessLauncher processLauncher) {
        packages = packageSource;
        processes = processRegistry;
        launcher = processLauncher;
    }

    public synchronized int bindServiceInstance(int callingPid, int callingUid, IBinder caller,
            IBinder activityToken, Intent intent, String resolvedType, IBinder connectionBinder,
            long flags, String instanceName, String callingPackage, int userId)
            throws RemoteException {
        if (userId != 0 || intent == null || connectionBinder == null) return 0;
        ApplicationProcessRegistry.AttachedApplication callingProcess =
                processes.requireCaller(callingPid, caller, callingPackage);
        if (callingProcess.uid >= 0 && callingProcess.uid != callingUid) {
            throw new SecurityException("Binder UID does not match attached application");
        }
        return bindServiceLocked(intent, resolvedType, connectionBinder, flags, instanceName,
                callingUid);
    }

    @Override
    public synchronized int bindService(Intent intent, IServiceConnection connection)
            throws RemoteException {
        if (intent == null || connection == null) return 0;
        return bindServiceLocked(intent, null, connection.asBinder(), 0, null,
                android.os.Process.SYSTEM_UID);
    }

    private int bindServiceLocked(Intent intent, String resolvedType, IBinder connectionBinder,
            long flags, String instanceName, int clientUid) throws RemoteException {
        ComponentName component = intent.getComponent();
        if (component == null) return 0;
        String packageName = component.getPackageName();
        ServiceInfo info = InstalledServiceInfo.service(packageName,
                packages.resolveInstalledPackage(packageName), component.getClassName());
        if (info == null || !info.enabled || info.applicationInfo == null) return 0;
        if (instanceName != null && instanceName.isEmpty()) {
            throw new IllegalArgumentException("Empty isolated service instance name");
        }
        boolean isolated = instanceName != null;
        if (isolated && (info.flags & ServiceInfo.FLAG_ISOLATED_PROCESS) == 0) {
            throw new SecurityException("Service is not declared isolatedProcess");
        }

        String key = serviceKey(component, instanceName);
        ServiceRecord service = services.get(key);
        if (service == null) {
            String processName = info.processName;
            if (processName == null || processName.isEmpty()) processName = packageName;
            if (isolated) processName = processName + ":" + instanceName;
            int uid = isolated ? allocateIsolatedUid() : info.applicationInfo.uid;
            service = new ServiceRecord(component, instanceName, processName, uid, isolated, info);
            services.put(key, service);
            servicesByToken.put(service.token, service);
        }

        Intent.FilterComparison comparison = new Intent.FilterComparison(intent);
        IntentBindRecord binding = service.bindings.get(comparison);
        if (binding == null) {
            binding = new IntentBindRecord(service, intent, resolvedType, nextSequence());
            service.bindings.put(comparison, binding);
        }
        IServiceConnection endpoint = IServiceConnection.Stub.asInterface(connectionBinder);
        if (endpoint == null) throw new IllegalArgumentException("Invalid service connection");
        ConnectionRecord connection = new ConnectionRecord(endpoint, binding, flags, clientUid);
        IBinder.DeathRecipient death = connectionDeaths.get(connection.connectionBinder);
        if (death == null) {
            death = new ConnectionDeathRecipient(connection.connectionBinder);
            try {
                connection.connectionBinder.linkToDeath(death, 0);
            } catch (RemoteException dead) {
                service.bindings.remove(new Intent.FilterComparison(binding.intent));
                if (service.bindings.isEmpty()) removeService(key, service);
                return 0;
            }
            connectionDeaths.put(connection.connectionBinder, death);
        }
        binding.connections.add(connection);
        connections.computeIfAbsent(connection.connectionBinder, unused -> new ArrayList<>())
                .add(connection);

        ApplicationProcessRegistry.AttachedApplication attached =
                processes.findAttached(packageName, service.processName, service.uid);
        if (attached != null) {
            attachAndSchedule(service, attached);
        } else if (!service.processRequested) {
            service.processRequested = true;
            try {
                launcher.start(packageName, service.processName, service.uid, service.isolated,
                        nextSequence());
            } catch (RuntimeException | Error error) {
                service.processRequested = false;
                removeConnection(connection);
                if (service.bindings.isEmpty()) removeService(key, service);
                throw error;
            }
        }
        if (binding.publicationReceived) {
            try {
                notifyConnected(connection, binding.publishedBinder);
            } catch (RemoteException deadClient) {
                removeConnectionsLocked(connection.connectionBinder, false);
                return 0;
            }
        }
        return 1;
    }

    @Override
    public synchronized boolean unbindService(IServiceConnection connection)
            throws RemoteException {
        return connection != null && unbindService(connection.asBinder());
    }

    public synchronized void onProcessAttached(
            ApplicationProcessRegistry.AttachedApplication attached) throws RemoteException {
        if (attached == null || attached.thread == null) return;
        boolean ownsLiveService = false;
        // A failed create/bind callback may retire this record or request a
        // replacement process. Iterate a snapshot so the owner-death
        // transition cannot invalidate this traversal.
        for (ServiceRecord service : new ArrayList<>(services.values())) {
            if (service.component.getPackageName().equals(attached.packageName)
                    && service.processName.equals(attached.processName)
                    && service.uid == attached.uid) {
                ownsLiveService |= attachAndSchedule(service, attached);
            }
        }
        // AOSP does not keep a freshly attached service-only process when the
        // binding that requested it vanished before attach completed. Ask the
        // real ActivityThread to leave its Looper instead of retaining an
        // inert Darwin child forever. Main/activity processes are outside this
        // service lifecycle decision.
        if (!ownsLiveService
                && attached.initialWork
                        == ApplicationProcessRegistry.InitialWork.BOUND_SERVICE) {
            IApplicationThread thread = IApplicationThread.Stub.asInterface(attached.thread);
            if (thread != null) thread.scheduleExit();
        }
    }

    /** Applies a process-death transition for one attached process snapshot. */
    public synchronized void onProcessGone(
            ApplicationProcessRegistry.AttachedApplication gone) {
        if (gone == null || gone.thread == null) return;
        for (ServiceRecord service : new ArrayList<>(servicesByToken.values())) {
            if (isOwner(service, gone)) {
                transitionAfterOwnerGone(service, gone.pid, gone.startSequence, gone.thread);
            }
        }
    }

    public synchronized void publishService(
            int callingPid, IBinder token, Intent intent, IBinder published) throws RemoteException {
        ServiceRecord service = servicesByToken.get(token);
        if (service == null) throw new IllegalArgumentException("Unknown service token");
        if (intent == null) throw new IllegalArgumentException("Missing service publication intent");
        ApplicationProcessRegistry.AttachedApplication caller =
                processes.requireAttachedProcess(callingPid);
        requireOwner(service, caller);
        IntentBindRecord binding = service.bindings.get(new Intent.FilterComparison(intent));
        if (binding != null && binding.bindScheduled && !binding.publicationReceived) {
            binding.publicationReceived = true;
            binding.publishedBinder = published;
            for (ConnectionRecord connection : new ArrayList<>(binding.connections)) {
                try {
                    notifyConnected(connection, published);
                } catch (RemoteException deadClient) {
                    removeConnectionsLocked(connection.connectionBinder, false);
                }
            }
        }
        finishExecutingCallback(service);
    }

    public synchronized void unbindFinished(
            int callingPid, IBinder token, Intent intent) throws RemoteException {
        ServiceRecord service = servicesByToken.get(token);
        if (service == null || intent == null) return;
        requireOwner(service, processes.requireAttachedProcess(callingPid));
        IntentBindRecord binding = service.bindings.get(new Intent.FilterComparison(intent));
        if (binding != null) {
            binding.unbindScheduled = false;
            binding.doRebind = true;
        }
        finishExecutingCallback(service);
    }

    public synchronized void serviceDoneExecuting(
            int callingUid, IBinder token, int type, int startId, int result, Intent intent)
            throws RemoteException {
        ServiceRecord service = servicesByToken.get(token);
        if (service == null) return;
        if (callingUid < 0 || service.uid != callingUid) {
            throw new SecurityException("Service completion came from another UID");
        }
        finishExecutingCallback(service);
    }

    public synchronized boolean unbindService(IBinder connectionBinder) throws RemoteException {
        return removeConnectionsLocked(connectionBinder, true);
    }

    private boolean removeConnectionsLocked(IBinder connectionBinder, boolean unlinkDeath)
            throws RemoteException {
        ArrayList<ConnectionRecord> records = connections.remove(connectionBinder);
        if (records == null || records.isEmpty()) return false;
        IBinder.DeathRecipient death = connectionDeaths.remove(connectionBinder);
        if (unlinkDeath && death != null) connectionBinder.unlinkToDeath(death, 0);
        for (ConnectionRecord connection : new ArrayList<>(records)) {
            IntentBindRecord binding = connection.binding;
            binding.connections.remove(connection);
            if (binding.connections.isEmpty()) {
                ServiceRecord service = binding.service;
                // scheduleBindService and publishService are asynchronous. AOSP
                // retains the IntentBindRecord and ServiceRecord token when the
                // final client disconnects, then lets the app finish onBind and
                // publish before the queued unbind runs. Removing either here
                // makes that ordinary race look like a forged publication.
                if (binding.bindScheduled && !binding.unbindScheduled
                        && service.applicationThread != null) {
                    try {
                        scheduleUnbind(service, binding);
                    } catch (RemoteException error) { // Includes DeadObjectException.
                        transitionAfterOwnerGone(service, service.ownerPid,
                                service.ownerStartSequence, service.applicationThread.asBinder());
                    }
                } else if (!binding.bindScheduled) {
                    service.bindings.remove(new Intent.FilterComparison(binding.intent));
                }
                retireIfUnowned(service);
            }
        }
        return true;
    }

    private final class ConnectionDeathRecipient implements IBinder.DeathRecipient {
        private final IBinder connectionBinder;

        ConnectionDeathRecipient(IBinder binder) {
            connectionBinder = binder;
        }

        @Override
        public void binderDied() {
            synchronized (ActiveServices.this) {
                try {
                    removeConnectionsLocked(connectionBinder, false);
                } catch (RemoteException impossible) {
                    throw new RuntimeException(impossible);
                }
            }
        }
    }

    private boolean attachAndSchedule(ServiceRecord service,
            ApplicationProcessRegistry.AttachedApplication attached) throws RemoteException {
        IApplicationThread thread = IApplicationThread.Stub.asInterface(attached.thread);
        if (thread == null) throw new IllegalStateException("Attached process has no app thread");
        service.applicationThread = thread;
        service.ownerPid = attached.pid;
        service.ownerStartSequence = attached.startSequence;
        service.processRequested = false;
        if (!hasConnections(service)) {
            retireIfUnowned(service);
            return false;
        }
        if (!service.createScheduled) {
            long identity = Binder.clearCallingIdentity();
            try {
                thread.scheduleCreateService(service.token, service.serviceInfo,
                        CompatibilityInfo.DEFAULT_COMPATIBILITY_INFO,
                        PROCESS_STATE_SERVICE);
            } catch (RemoteException error) { // Includes DeadObjectException.
                transitionAfterOwnerGone(service, service.ownerPid,
                        service.ownerStartSequence, attached.thread);
                return false;
            } finally {
                Binder.restoreCallingIdentity(identity);
            }
            service.createScheduled = true;
            service.executingCallbacks++;
        }
        for (IntentBindRecord binding : service.bindings.values()) {
            if (!binding.connections.isEmpty() && !binding.bindScheduled) {
                long identity = Binder.clearCallingIdentity();
                try {
                    thread.scheduleBindService(service.token, binding.intent, binding.doRebind,
                            PROCESS_STATE_SERVICE, binding.bindSequence);
                } catch (RemoteException error) { // Includes DeadObjectException.
                    transitionAfterOwnerGone(service, service.ownerPid,
                            service.ownerStartSequence, attached.thread);
                    return false;
                } finally {
                    Binder.restoreCallingIdentity(identity);
                }
                binding.requested = true;
                binding.bindScheduled = true;
                binding.hasBound = true;
                binding.unbindScheduled = false;
                binding.doRebind = false;
                service.executingCallbacks++;
            }
        }
        return hasConnections(service);
    }

    private static void notifyConnected(ConnectionRecord connection, IBinder binder)
            throws RemoteException {
        long identity = Binder.clearCallingIdentity();
        try {
            connection.connection.connected(connection.binding.service.component, binder, false);
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    private void removeConnection(ConnectionRecord connection) {
        connection.binding.connections.remove(connection);
        ArrayList<ConnectionRecord> records = connections.get(connection.connectionBinder);
        if (records != null) {
            records.remove(connection);
            if (records.isEmpty()) {
                connections.remove(connection.connectionBinder);
                IBinder.DeathRecipient death = connectionDeaths.remove(
                        connection.connectionBinder);
                if (death != null) connection.connectionBinder.unlinkToDeath(death, 0);
            }
        }
        if (connection.binding.connections.isEmpty()) {
            connection.binding.service.bindings.remove(
                    new Intent.FilterComparison(connection.binding.intent));
        }
    }

    private void removeService(String key, ServiceRecord service) {
        services.remove(key, service);
        servicesByToken.remove(service.token, service);
    }

    private void scheduleUnbind(ServiceRecord service, IntentBindRecord binding)
            throws RemoteException {
        long identity = Binder.clearCallingIdentity();
        try {
            service.applicationThread.scheduleUnbindService(service.token, binding.intent);
        } catch (RemoteException error) { // Includes DeadObjectException.
            transitionAfterOwnerGone(service, service.ownerPid,
                    service.ownerStartSequence, service.applicationThread.asBinder());
            return;
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
        binding.unbindScheduled = true;
        binding.hasBound = false;
        service.executingCallbacks++;
    }

    private void retireIfUnowned(ServiceRecord service) throws RemoteException {
        if (hasConnections(service) || service.retiring) return;
        service.retiring = true;
        services.remove(serviceKey(service.component, service.instanceName), service);
        maybeStopOrRemove(service);
    }

    private void finishExecutingCallback(ServiceRecord service) throws RemoteException {
        if (service.executingCallbacks > 0) service.executingCallbacks--;
        maybeStopOrRemove(service);
    }

    private void maybeStopOrRemove(ServiceRecord service) throws RemoteException {
        if (!service.retiring || service.executingCallbacks != 0) return;
        if (service.createScheduled && !service.stopScheduled
                && service.applicationThread != null) {
            long identity = Binder.clearCallingIdentity();
            try {
                service.applicationThread.scheduleStopService(service.token);
            } catch (RemoteException error) { // Includes DeadObjectException.
                transitionAfterOwnerGone(service, service.ownerPid,
                        service.ownerStartSequence, service.applicationThread.asBinder());
                return;
            } finally {
                Binder.restoreCallingIdentity(identity);
            }
            service.stopScheduled = true;
            service.executingCallbacks++;
            return;
        }
        if (!service.createScheduled || service.stopScheduled) {
            servicesByToken.remove(service.token, service);
            service.bindings.clear();
        }
    }

    private static boolean hasConnections(ServiceRecord service) {
        for (IntentBindRecord binding : service.bindings.values()) {
            if (!binding.connections.isEmpty()) return true;
        }
        return false;
    }

    private static boolean isOwner(ServiceRecord service,
            ApplicationProcessRegistry.AttachedApplication attached) {
        return service.ownerPid == attached.pid
                && service.ownerStartSequence == attached.startSequence
                && service.uid == attached.uid
                && service.applicationThread != null
                && service.applicationThread.asBinder().equals(attached.thread);
    }

    /**
     * Clears callbacks belonging to a dead owner while retaining live client
     * records. Existing clients request a fresh process; an ownerless service
     * is retired without sending more Binder work to the dead process.
     */
    private void transitionAfterOwnerGone(ServiceRecord service, int pid,
            long startSequence, IBinder threadBinder) {
        if (service.applicationThread == null
                || service.ownerPid != pid
                || service.ownerStartSequence != startSequence
                || !service.applicationThread.asBinder().equals(threadBinder)) return;

        service.applicationThread = null;
        service.ownerPid = 0;
        service.ownerStartSequence = -1;
        service.createScheduled = false;
        service.stopScheduled = false;
        service.processRequested = false;
        service.executingCallbacks = 0;

        ArrayList<Intent.FilterComparison> emptyBindings = new ArrayList<>();
        for (java.util.Map.Entry<Intent.FilterComparison, IntentBindRecord> entry
                : service.bindings.entrySet()) {
            IntentBindRecord binding = entry.getValue();
            binding.bindScheduled = false;
            binding.unbindScheduled = false;
            binding.requested = false;
            binding.hasBound = false;
            binding.publicationReceived = false;
            binding.publishedBinder = null;
            binding.doRebind = false;
            if (binding.connections.isEmpty()) emptyBindings.add(entry.getKey());
        }
        for (Intent.FilterComparison key : emptyBindings) {
            IntentBindRecord binding = service.bindings.remove(key);
            removeIndexedConnections(binding);
        }

        if (hasConnections(service)) {
            service.retiring = false;
            service.processRequested = true;
            try {
                launcher.start(service.component.getPackageName(), service.processName, service.uid,
                        service.isolated, nextSequence());
            } catch (RuntimeException | Error error) {
                service.processRequested = false;
            }
        } else {
            service.retiring = true;
            services.remove(serviceKey(service.component, service.instanceName), service);
            servicesByToken.remove(service.token, service);
            service.bindings.clear();
        }
    }

    private void removeIndexedConnections(IntentBindRecord binding) {
        if (binding == null) return;
        ArrayList<IBinder> emptyKeys = new ArrayList<>();
        for (java.util.Map.Entry<IBinder, ArrayList<ConnectionRecord>> entry
                : connections.entrySet()) {
            entry.getValue().removeIf(connection -> connection.binding == binding);
            if (entry.getValue().isEmpty()) emptyKeys.add(entry.getKey());
        }
        for (IBinder key : emptyKeys) connections.remove(key);
    }

    private static void requireOwner(ServiceRecord service,
            ApplicationProcessRegistry.AttachedApplication caller) {
        if (service.ownerPid != caller.pid
                || service.ownerStartSequence != caller.startSequence
                || service.applicationThread == null
                || !service.applicationThread.asBinder().equals(caller.thread)
                || service.uid != caller.uid) {
            throw new SecurityException("Service callback came from another process incarnation");
        }
    }

    private long nextSequence() {
        if (nextSequence == Long.MAX_VALUE) throw new IllegalStateException("Sequence exhausted");
        return nextSequence++;
    }

    private int allocateIsolatedUid() {
        for (int count = 0; count <= LAST_ISOLATED_UID - FIRST_ISOLATED_UID; count++) {
            int candidate = nextIsolatedUid;
            nextIsolatedUid = candidate == LAST_ISOLATED_UID ? FIRST_ISOLATED_UID : candidate + 1;
            boolean used = false;
            for (ServiceRecord service : services.values()) {
                if (service.isolated && service.uid == candidate) {
                    used = true;
                    break;
                }
            }
            if (!used) return candidate;
        }
        throw new IllegalStateException("Isolated UID range exhausted");
    }

    private static String serviceKey(ComponentName component, String instanceName) {
        return component.flattenToString() + "\u0000" + (instanceName == null ? "" : instanceName);
    }
}
