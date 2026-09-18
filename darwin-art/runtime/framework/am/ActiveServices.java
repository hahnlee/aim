package dev.darwinart.runtime.am;

import android.app.IApplicationThread;
import android.app.IServiceConnection;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteException;
import dev.darwinart.runtime.pm.InstalledServiceInfo;
import dev.darwinart.runtime.pm.PackageRecords;
import java.util.ArrayList;
import java.util.HashMap;

/** Android service lifecycle owner for system_server's ActivityManager endpoint. */
public final class ActiveServices implements SystemServiceBindings {
    private static final int FIRST_ISOLATED_UID = 99000;
    private static final int LAST_ISOLATED_UID = 99999;

    private final PackageRecords.Source packages;
    private final ApplicationProcessRegistry processes;
    private final ServiceProcessLaunchController processLaunches;
    private final HashMap<String, ServiceRecord> services = new HashMap<>();
    private final HashMap<IBinder, ServiceRecord> servicesByToken = new HashMap<>();
    /** Guest callback capabilities; entries exist only for the current live lane. */
    private final HashMap<IBinder, ServiceRecord.LifecycleLane> lifecycleByToken = new HashMap<>();
    private final ServiceConnectionIndex connectionIndex = new ServiceConnectionIndex();
    private final ServiceNotificationController notifications =
            new ServiceNotificationController(this, connectionIndex,
                    this::canNotifyConnectionLocked, this::removeConnection);
    private final ServiceLifecycleController lifecycleDispatcher =
            new ServiceLifecycleController(this);
    private final ServiceConnectionResourceController connectionResources =
            new ServiceConnectionResourceController(this);
    private long nextSequence = 1;
    private int nextIsolatedUid = FIRST_ISOLATED_UID;

    ActiveServices(PackageRecords.Source packageSource, ApplicationProcessRegistry processRegistry,
            ProcessLaunchTransport processLauncher) {
        packages = packageSource;
        processes = processRegistry;
        processLaunches = new ServiceProcessLaunchController(this,
                new ServiceProcessLaunchController.StatePort() {
                    @Override public ArrayList<ServiceRecord> servicesLocked() {
                        return new ArrayList<>(servicesByToken.values());
                    }
                    @Override public long nextSequenceLocked() { return nextSequence(); }
                    @Override public void launchFailedLocked(ConnectionRecord initiating, long revision,
                            Throwable failure) {
                        if (initiating == null || !connectionIndex.contains(initiating)
                                || servicesByToken.get(initiating.binding.service.canonicalToken)
                                    != initiating.binding.service) return;
                        ServiceRecord service = initiating.binding.service;
                        if (service.ownerRevision != revision) return;
                        if (service.applicationThread != null && processes.hasCallerIncarnation(
                                service.ownerPid, service.ownerStartSequence,
                                service.applicationThread.asBinder(), service.uid)) return;
                        try { removeConnection(initiating); }
                        catch (RemoteException cleanup) { failure.addSuppressed(cleanup); }
                    }
                }, processes, processLauncher);
    }

    public int bindServiceInstance(int callingPid, int callingUid, IBinder caller,
            IBinder activityToken, Intent intent, String resolvedType, IBinder connectionBinder,
            long flags, String instanceName, String callingPackage, int userId)
            throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            PendingServiceConnectionBind pending;
            synchronized (this) {
                if (userId != 0 || intent == null || connectionBinder == null) return 0;
                ApplicationProcessRegistry.AttachedApplication callingProcess =
                        processes.requireCaller(callingPid, caller, callingPackage);
                if (callingProcess.uid >= 0 && callingProcess.uid != callingUid) {
                    throw new SecurityException("Binder UID does not match attached application");
                }
                pending = bindServiceLocked(intent, resolvedType, connectionBinder, flags, instanceName,
                        callingUid, ServiceConnectionOwner.application(callingProcess));
            }
            return finishPendingBind(pending);
        }
    }

    @Override
    public int bindService(Intent intent, IServiceConnection connection)
            throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            PendingServiceConnectionBind pending;
            synchronized (this) {
                if (intent == null || connection == null) return 0;
                pending = bindServiceLocked(intent, null, connection.asBinder(), 0, null,
                        android.os.Process.SYSTEM_UID, ServiceConnectionOwner.system());
            }
            return finishPendingBind(pending);
        }
    }

    private PendingServiceConnectionBind bindServiceLocked(Intent intent, String resolvedType,
            IBinder connectionBinder, long flags, String instanceName, int clientUid,
            ServiceConnectionOwner owner) throws RemoteException {
        ComponentName component = intent.getComponent();
        if (component == null) return null;
        String packageName = component.getPackageName();
        ServiceInfo info = InstalledServiceInfo.service(packageName,
                packages.resolveInstalledPackage(packageName), component.getClassName());
        if (info == null || !info.enabled || info.applicationInfo == null) return null;
        if (instanceName != null && instanceName.isEmpty()) {
            throw new IllegalArgumentException("Empty isolated service instance name");
        }
        boolean isolated = instanceName != null;
        if (isolated && (info.flags & ServiceInfo.FLAG_ISOLATED_PROCESS) == 0) {
            throw new SecurityException("Service is not declared isolatedProcess");
        }

        String key = serviceKey(component, instanceName);
        ServiceRecord service = services.get(key);
        boolean newService = service == null;
        if (service == null) {
            String processName = info.processName;
            if (processName == null || processName.isEmpty()) processName = packageName;
            if (isolated) processName = processName + ":" + instanceName;
            int uid = isolated ? allocateIsolatedUid() : info.applicationInfo.uid;
            service = new ServiceRecord(component, instanceName, processName, uid, isolated, info);
            services.put(key, service);
            servicesByToken.put(service.canonicalToken, service);
        }

        Intent.FilterComparison comparison = new Intent.FilterComparison(intent);
        IntentBindRecord binding = service.bindings.get(comparison);
        if (binding == null) {
            binding = new IntentBindRecord(service, intent, resolvedType, nextSequence());
            service.bindings.put(comparison, binding);
        }
        IServiceConnection endpoint = IServiceConnection.Stub.asInterface(connectionBinder);
        if (endpoint == null) throw new IllegalArgumentException("Invalid service connection");
        ConnectionRecord connection = new ConnectionRecord(endpoint, binding, flags, clientUid, owner);
        IBinder.DeathRecipient existingDeath = connectionIndex.death(connection.connectionBinder);
        if (existingDeath instanceof ServiceConnectionDeathRegistration
                && ((ServiceConnectionDeathRegistration) existingDeath).state
                == ServiceConnectionDeathRegistration.State.LINKING) {
            if (!binding.hasAdmittedConnections() && binding.pendingBinds == 0) {
                service.bindings.remove(comparison, binding);
            }
            if (newService && service.bindings.isEmpty() && service.pendingBinds == 0) {
                removeService(key, service);
            }
            return null;
        }
        connectionIndex.add(connection);
        connection.pendingAdmission = true;
        binding.pendingBinds++;
        service.pendingBinds++;
        ServiceConnectionDeathRegistration registration = null;
        if (existingDeath == null) {
            registration = new ServiceConnectionDeathRegistration(
                    connection.connectionBinder, this::onConnectionDeath);
            connectionIndex.setDeath(connection.connectionBinder, registration);
        }
        return new PendingServiceConnectionBind(
                key, service, binding, connection,
                registration == null ? existingDeath : registration, registration);
    }

    /** Completes the link outside AMS, then publishes lifecycle demand only on success. */
    private int finishPendingBind(PendingServiceConnectionBind pending) throws RemoteException {
        if (pending == null) return 0;
        Throwable linkFailure = null;
        if (pending.registration != null) {
            try {
                pending.registration.linkOutsideLock();
            } catch (RemoteException | RuntimeException | Error failure) {
                linkFailure = failure;
            }
        }
        int result = 0;
        synchronized (this) {
            ServiceConnectionDeathRegistration registration = pending.registration;
            if (registration != null) {
                boolean unlink = linkFailure == null
                        ? registration.finishLinkLocked()
                        : registration.failLinkLocked(!(linkFailure instanceof RemoteException));
                if (unlink) connectionResources.enqueueLocked(registration);
            }
            boolean admitted = linkFailure == null
                    && (registration == null || registration.isLinkedLocked())
                    && connectionIndex.contains(pending.connection)
                    && connectionIndex.death(pending.connection.connectionBinder)
                        == pending.expectedDeath
                    && ownerStillAdmissibleLocked(pending);
            if (admitted) {
                pending.connection.admitted = true;
                releasePendingPinLocked(pending.connection);
                if (!pending.service.stopScheduled) pending.service.retiring = false;
                result = finishAdmittedBindLocked(pending);
            } else {
                rejectPendingBindLocked(pending);
            }
        }
        Throwable cleanupFailure = connectionResources.drain();
        Throwable notificationFailure = result == 0 ? null
                : notifications.dispatchOne(pending.connection);
        if (linkFailure != null) {
            if (cleanupFailure != null && cleanupFailure != linkFailure) {
                linkFailure.addSuppressed(cleanupFailure);
            }
            if (notificationFailure != null && notificationFailure != linkFailure) {
                linkFailure.addSuppressed(notificationFailure);
            }
            if (linkFailure instanceof RemoteException) {
                if (cleanupFailure != null) throw (RemoteException) linkFailure;
                return 0;
            }
            rethrow(linkFailure);
        }
        if (cleanupFailure != null) {
            if (notificationFailure != null && notificationFailure != cleanupFailure) {
                cleanupFailure.addSuppressed(notificationFailure);
            }
            rethrow(cleanupFailure);
        }
        if (notificationFailure instanceof RemoteException) {
            if (notificationFailure.getSuppressed().length != 0) {
                throw (RemoteException) notificationFailure;
            }
            return 0;
        }
        if (notificationFailure != null) rethrow(notificationFailure);
        return result;
    }

    private boolean ownerStillAdmissibleLocked(PendingServiceConnectionBind pending) {
        return pending.service.bindings.get(new Intent.FilterComparison(pending.binding.intent))
                        == pending.binding
                && services.get(pending.serviceKey) == pending.service
                && servicesByToken.get(pending.service.canonicalToken) == pending.service
                && !pending.service.stopScheduled
                && pending.connection.owner.isCurrent(processes);
    }

    /** Caller owns the ActiveServices monitor. */
    private int finishAdmittedBindLocked(PendingServiceConnectionBind pending)
            throws RemoteException {
        ConnectionRecord connection = pending.connection;
        ServiceRecord service = pending.service;
        IntentBindRecord binding = pending.binding;
        ApplicationProcessRegistry.AttachedApplication attached =
                processes.findAttached(service.component.getPackageName(), service.processName,
                        service.uid);
        if (attached != null) {
            attachAndSchedule(service, attached);
        } else {
            processLaunches.requestLocked(service, connection);
        }
        return 1;
    }

    /** Caller owns the ActiveServices monitor. */
    private void rejectPendingBindLocked(PendingServiceConnectionBind pending)
            throws RemoteException {
        if (pending.connection.admitted) return;
        ServiceConnectionIndex.Detached detached = connectionIndex.remove(pending.connection);
        releasePendingPinLocked(pending.connection);
        unlinkDetached(detached);
        settleDetached(detached);
        planLifecycleLocked(pending.service);
    }

    private void releasePendingPinLocked(ConnectionRecord connection) {
        if (!connection.pendingAdmission) return;
        connection.pendingAdmission = false;
        if (connection.binding.pendingBinds <= 0 || connection.binding.service.pendingBinds <= 0) {
            throw new IllegalStateException("Unowned pending service connection pin");
        }
        connection.binding.pendingBinds--;
        connection.binding.service.pendingBinds--;
    }

    @Override
    public boolean unbindService(IServiceConnection connection) throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            synchronized (this) {
                return connection != null && removeConnectionsLocked(connection.asBinder(), true);
            }
        }
    }

    public void onProcessAttached(
            ApplicationProcessRegistry.AttachedApplication attached) throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            synchronized (this) {
                if (attached == null || attached.thread == null) return;
                // A failed create/bind callback may retire this record or request a
                // replacement process. Iterate a snapshot so the owner-death
                // transition cannot invalidate this traversal.
                for (ServiceRecord service : new ArrayList<>(services.values())) {
                    if (service.component.getPackageName().equals(attached.packageName)
                            && service.processName.equals(attached.processName)
                            && service.uid == attached.uid) {
                        attachAndSchedule(service, attached);
                    }
                }
            }
        }
        // Attachment without pending service work is not permission to quit
        // ActivityThread's main Looper. Cached/isolated process retirement is
        // AMS process policy, not a service callback or a BOUND_SERVICE test.
    }

    /** Applies a process-death transition for one attached process snapshot. */
    public void onProcessGone(
            ApplicationProcessRegistry.AttachedApplication gone) {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            synchronized (this) {
                if (gone == null || gone.thread == null) return;
                processLaunches.processGoneLocked(gone);
                // Remove every connection made by this exact client before deciding
                // which services have surviving demand. The ledger detaches all rows
                // atomically; lifecycle work cannot observe a partially detached client.
                ServiceConnectionIndex.Detached detached = connectionIndex.detachClient(gone);
                unlinkDetached(detached);
                for (ServiceRecord service : new ArrayList<>(servicesByToken.values())) {
                    if (isOwner(service, gone)) {
                        transitionAfterOwnerGone(service, gone.pid, gone.startSequence, gone.thread);
                    }
                }
                try {
                    settleDetached(detached);
                } catch (RemoteException impossible) {
                    throw new IllegalStateException("Service disconnect failed", impossible);
                }
            }
        }
    }

    public void publishService(
            int callingPid, IBinder token, Intent intent, IBinder published) throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            IntentBindRecord publishedBinding;
            ServiceRecord service;
            ApplicationProcessRegistry.AttachedApplication caller;
            synchronized (this) {
                ServiceRecord.LifecycleLane lane = requireLiveLifecycleLaneLocked(token);
                service = lane.service;
                if (intent == null) throw new IllegalArgumentException("Missing service publication intent");
                if (callingPid <= 0) {
                    throw new SecurityException("Synchronous service publication needs owner PID");
                }
                caller = processes.requireAttachedProcess(callingPid);
                requireOwner(service, caller);
                IntentBindRecord binding = service.bindings.get(new Intent.FilterComparison(intent));
                if (binding != null && binding.bindScheduled && !binding.publicationReceived) {
                    notifications.publishLocked(binding, published);
                }
                // An outbound connected callback may replace the process owner. The
                // old publication must not consume the replacement's reserved callback.
                if (!isOwner(service, caller) || !isCurrentLifecycleLaneLocked(lane)
                        || (binding != null
                        && service.bindings.get(new Intent.FilterComparison(intent)) != binding)) return;
                lifecycleDispatcher.publishedLocked(service, binding);
                publishedBinding = binding;
            }
            if (publishedBinding != null) notifications.dispatchBinding(publishedBinding);
        }
    }

    public void unbindFinished(
            int callingPid, IBinder token, Intent intent) throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            synchronized (this) {
                ServiceRecord.LifecycleLane lane = requireLiveLifecycleLaneLocked(token);
                ServiceRecord service = lane.service;
                if (intent == null) return;
                if (callingPid <= 0) {
                    throw new SecurityException("Synchronous unbind completion needs owner PID");
                }
                requireOwner(service, processes.requireAttachedProcess(callingPid));
                IntentBindRecord binding = service.bindings.get(new Intent.FilterComparison(intent));
                lifecycleDispatcher.unbindFinishedLocked(service, binding, true);
            }
        }
    }

    public void serviceDoneExecuting(
            int callingPid, int callingUid, IBinder token, int type, int startId, int result,
            Intent intent)
            throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            synchronized (this) {
                ServiceRecord.LifecycleLane lane = requireLiveLifecycleLaneLocked(token);
                ServiceRecord service = lane.service;
                if (callingUid < 0 || service.uid != callingUid
                        || lane.attachedOwner.uid != callingUid) {
                    throw new SecurityException("Service completion came from another UID");
                }
                if (callingPid == 0) {
                    // Binder does not expose a sender PID for FLAG_ONEWAY calls. The
                    // capability still carries the exact app incarnation; retirement
                    // removes that tuple from the registry before replacement.
                    if (!processes.hasCallerIncarnation(lane.ownerPid,
                            lane.ownerStartSequence, lane.ownerThread, callingUid)) {
                        throw new SecurityException("Service completion owner is retired");
                    }
                } else {
                    if (callingPid < 0 || callingPid != lane.ownerPid) {
                        throw new SecurityException("Service completion came from another process");
                    }
                    requireOwner(service, processes.requireAttachedProcess(callingPid));
                }
                lifecycleDispatcher.doneLocked(service, type, intent);
            }
        }
    }

    public boolean unbindService(IBinder connectionBinder) throws RemoteException {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            synchronized (this) {
                return removeConnectionsLocked(connectionBinder, true);
            }
        }
    }

    private boolean removeConnectionsLocked(IBinder connectionBinder, boolean unlinkDeath)
            throws RemoteException {
        ServiceConnectionIndex.Detached detached = connectionIndex.detachBinder(connectionBinder);
        if (detached.isEmpty()) return false;
        if (unlinkDeath) unlinkDetached(detached);
        settleDetached(detached);
        return true;
    }

    private void unlinkDetached(ServiceConnectionIndex.Detached detached) {
        for (ServiceConnectionIndex.DeathPin pin : detached.deaths()) {
            if (pin.recipient instanceof ServiceConnectionDeathRegistration) {
                ServiceConnectionDeathRegistration registration =
                        (ServiceConnectionDeathRegistration) pin.recipient;
                if (registration.claimUnlinkLocked()) connectionResources.enqueueLocked(registration);
            } else {
                throw new IllegalStateException("Unknown service connection death owner");
            }
        }
    }

    private void settleDetached(ServiceConnectionIndex.Detached detached) throws RemoteException {
        for (ConnectionRecord connection : detached.records()) {
            notifications.detachLocked(connection);
            releasePendingPinLocked(connection);
            IntentBindRecord binding = connection.binding;
            ServiceRecord current = binding.service;
            if (servicesByToken.get(current.canonicalToken) != current
                    || current.bindings.get(new Intent.FilterComparison(binding.intent)) != binding) {
                continue;
            }
            if (!binding.hasAdmittedConnections()) {
                ServiceRecord service = binding.service;
                // scheduleBindService and publishService are asynchronous. AOSP
                // retains the IntentBindRecord and ServiceRecord token when the
                // final client disconnects, then lets the app finish onBind and
                // publish before the queued unbind runs. Removing either here
                // makes that ordinary race look like a forged publication.
                if (binding.bindScheduled && binding.hasBound && !binding.unbindScheduled
                        && service.applicationThread != null) {
                    binding.unbindRequested = true;
                    planLifecycleLocked(service);
                } else if (!binding.bindScheduled && binding.pendingBinds == 0) {
                    service.bindings.remove(new Intent.FilterComparison(binding.intent), binding);
                }
                retireIfUnowned(service);
            }
            planLifecycleLocked(current);
        }
    }

    private void onConnectionDeath(ServiceConnectionDeathRegistration registration) {
        try (DeferredTailScope tails = new DeferredTailScope()) {
            synchronized (this) {
                registration.dead = true;
                if (connectionIndex.death(registration.binder) != registration) return;
                try {
                    removeConnectionsLocked(registration.binder, false);
                } catch (RemoteException impossible) {
                    throw new RuntimeException(impossible);
                }
            }
        }
    }

    private boolean attachAndSchedule(ServiceRecord service,
            ApplicationProcessRegistry.AttachedApplication attached) throws RemoteException {
        if (!processes.hasCallerIncarnation(attached.pid, attached.startSequence,
                attached.thread, attached.uid)) return false;
        IApplicationThread thread = IApplicationThread.Stub.asInterface(attached.thread);
        if (thread == null) throw new IllegalStateException("Attached process has no app thread");
        if (service.lifecycleLane != null
                && !service.lifecycleLane.closed
                && (service.ownerPid != attached.pid
                    || service.ownerStartSequence != attached.startSequence
                    || !service.lifecycleLane.ownerThread.equals(attached.thread))) {
            return false;
        }
        if ((service.lifecycleLane == null || service.lifecycleLane.closed)
                && service.ownerRevision == Long.MAX_VALUE) {
            throw new IllegalStateException("Service owner revision exhausted");
        }
        service.applicationThread = thread;
        service.ownerPid = attached.pid;
        service.ownerStartSequence = attached.startSequence;
        if (service.lifecycleLane == null || service.lifecycleLane.closed) {
            service.ownerRevision++;
            ServiceRecord.LifecycleLane lane = new ServiceRecord.LifecycleLane(service, attached);
            service.lifecycleLane = lane;
            // The canonical ServiceRecord identity stays system-server-only.  Every
            // process incarnation receives a new callback capability, so an old
            // Binder token cannot reach a replacement with the same PID/UID.
            service.token = lane.callbackToken;
            lifecycleByToken.put(lane.callbackToken, lane);
        }
        service.processRequested = false;
        if (!hasConnections(service)) {
            retireIfUnowned(service);
            return false;
        }
        planLifecycleLocked(service);
        return hasConnections(service);
    }

    /** Policy admission only; notification transport runs after this monitor is released. */
    private boolean canNotifyConnectionLocked(ConnectionRecord connection,
            IBinder.DeathRecipient recipient) {
        ServiceRecord service = connection.binding.service;
        ServiceRecord.LifecycleLane lane = service.lifecycleLane;
        return connection.admitted && connectionIndex.contains(connection)
                && recipient instanceof ServiceConnectionDeathRegistration
                && connectionIndex.death(connection.connectionBinder) == recipient
                && ((ServiceConnectionDeathRegistration) recipient).isLinkedLocked()
                && connection.owner.isCurrent(processes)
                && servicesByToken.get(service.canonicalToken) == service
                && services.get(serviceKey(service.component, service.instanceName)) == service
                && service.bindings.get(new Intent.FilterComparison(connection.binding.intent))
                        == connection.binding
                && lane != null && processes.hasCallerIncarnation(lane.ownerPid,
                        lane.ownerStartSequence, lane.ownerThread, service.uid);
    }

    private void removeConnection(ConnectionRecord connection) throws RemoteException {
        ServiceConnectionIndex.Detached detached = connectionIndex.remove(connection);
        unlinkDetached(detached);
        settleDetached(detached);
    }

    private void removeService(String key, ServiceRecord service) {
        services.remove(key, service);
        servicesByToken.remove(service.canonicalToken, service);
        invalidateLifecycleCapabilityLocked(service.lifecycleLane);
    }

    private void retireIfUnowned(ServiceRecord service) throws RemoteException {
        if (hasConnections(service) || service.retiring) return;
        service.retiring = true;
        planLifecycleLocked(service);
    }

    private void planLifecycleLocked(ServiceRecord service) {
        lifecycleDispatcher.markDirtyLocked(service);
    }

    boolean lifecycleCanonicalLocked(ServiceRecord service) {
        return servicesByToken.get(service.canonicalToken) == service;
    }

    void removeLifecycleCatalogKeyLocked(ServiceRecord service) {
        services.remove(serviceKey(service.component, service.instanceName), service);
    }

    void removeLifecycleServiceLocked(ServiceRecord service) {
        removeLifecycleCatalogKeyLocked(service);
        servicesByToken.remove(service.canonicalToken, service);
        service.bindings.clear();
        if (service.lifecycleLane != null) {
            invalidateLifecycleCapabilityLocked(service.lifecycleLane);
            lifecycleDispatcher.closeLocked(service.lifecycleLane);
        }
    }

    /** Called by the dispatcher with ActiveServices held after an IPC failure. */
    void lifecycleTransportFailedLocked(ServiceLifecycleOperation operation, Throwable failure,
            boolean terminalOwner) {
        if (!operation.matchesOwner()) return;
        operation.lane.transportSealed = true;
        operation.lane.transportFailure = failure;
        if (!terminalOwner) {
            return;
        }
        ApplicationProcessRegistry.AttachedApplication gone = processes.retireAttached(
                operation.ownerPid, operation.ownerSequence, operation.ownerThread);
        onProcessGone(gone == null ? operation.lane.attachedOwner : gone);
    }

    private static boolean hasConnections(ServiceRecord service) {
        for (IntentBindRecord binding : service.bindings.values()) {
            if (binding.hasAdmittedConnections()) return true;
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
     * Resolves and authenticates a guest callback capability while ActiveServices is held.
     * The registry tuple check is deliberate: Binder token identity alone must not survive
     * process retirement or authorize a same-PID replacement.
     */
    private ServiceRecord.LifecycleLane requireLiveLifecycleLaneLocked(IBinder token) {
        ServiceRecord.LifecycleLane lane = token == null ? null : lifecycleByToken.get(token);
        if (lane == null || lane.closed || lane.service.lifecycleLane != lane
                || servicesByToken.get(lane.service.canonicalToken) != lane.service
                || !processes.hasCallerIncarnation(lane.ownerPid, lane.ownerStartSequence,
                        lane.ownerThread, lane.service.uid)) {
            throw new SecurityException("Unknown or retired service callback capability");
        }
        return lane;
    }

    private boolean isCurrentLifecycleLaneLocked(ServiceRecord.LifecycleLane lane) {
        return lane != null && !lane.closed && lane.service.lifecycleLane == lane
                && lifecycleByToken.get(lane.callbackToken) == lane
                && servicesByToken.get(lane.service.canonicalToken) == lane.service;
    }

    private void invalidateLifecycleCapabilityLocked(ServiceRecord.LifecycleLane lane) {
        if (lane == null) return;
        lifecycleByToken.remove(lane.callbackToken, lane);
        if (lane.service.lifecycleLane == lane) lane.service.token = lane.service.canonicalToken;
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

        ServiceRecord.LifecycleLane oldLane = service.lifecycleLane;
        if (oldLane != null) {
            invalidateLifecycleCapabilityLocked(oldLane);
            lifecycleDispatcher.closeLocked(oldLane);
        }
        service.lifecycleLane = null;

        service.applicationThread = null;
        service.ownerPid = 0;
        service.ownerStartSequence = -1;
        service.createScheduled = false;
        service.stopScheduled = false;
        service.processRequested = false;
        service.executingCallbacks = 0;
        service.createCallbackPending = false;
        service.stopCallbackPending = false;

        ArrayList<Intent.FilterComparison> emptyBindings = new ArrayList<>();
        for (java.util.Map.Entry<Intent.FilterComparison, IntentBindRecord> entry
                : service.bindings.entrySet()) {
            IntentBindRecord binding = entry.getValue();
            binding.bindScheduled = false;
            binding.unbindRequested = false;
            binding.unbindScheduled = false;
            binding.requested = false;
            binding.hasBound = false;
            binding.publicationReceived = false;
            notifications.invalidateLocked(binding);
            binding.publishedBinder = null;
            binding.doRebind = false;
            binding.bindCallbackPending = false;
            binding.rebindCallbackPending = false;
            if (!binding.hasAdmittedConnections() && binding.pendingBinds == 0) {
                emptyBindings.add(entry.getKey());
            }
        }
        for (Intent.FilterComparison key : emptyBindings) {
            IntentBindRecord binding = service.bindings.remove(key);
            removeIndexedConnections(binding);
        }

        if (hasConnections(service)) {
            service.retiring = false;
            ApplicationProcessRegistry.AttachedApplication replacement = processes.findAttached(
                    service.component.getPackageName(), service.processName, service.uid);
            if (replacement != null) {
                try { attachAndSchedule(service, replacement); }
                catch (RemoteException failure) {
                    throw new IllegalStateException("Replacement owner admission failed", failure);
                }
            } else {
                processLaunches.requestLocked(service, null);
            }
        } else {
            service.retiring = true;
            planLifecycleLocked(service);
        }
    }

    private void removeIndexedConnections(IntentBindRecord binding) {
        unlinkDetached(connectionIndex.detachBinding(binding));
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

    /** Performs queued Binder resource tails with no ActiveServices lock held. */
    private void drainDeferredTails() {
        if (Thread.holdsLock(this)) return;
        Throwable failure = null;
        long epoch = processLaunches.beginDrain();
        java.util.IdentityHashMap<ServiceConnectionDeathRegistration, Boolean> triedUnlinks =
                new java.util.IdentityHashMap<>();
        boolean progressed;
        do {
            progressed = false;
            Throwable unlinkFailure = connectionResources.drain(triedUnlinks);
            if (unlinkFailure != null) {
                if (failure == null) failure = unlinkFailure;
                else if (failure != unlinkFailure) failure.addSuppressed(unlinkFailure);
            }
            try {
                progressed |= processLaunches.drain(epoch);
            } catch (RuntimeException | Error next) {
                progressed = true;
                if (failure == null) failure = next;
                else if (failure != next) failure.addSuppressed(next);
            }
            try {
                progressed |= lifecycleDispatcher.drain();
            } catch (RuntimeException | Error next) {
                progressed = true;
                if (failure == null) failure = next;
                else if (failure != next) failure.addSuppressed(next);
            }
        } while (progressed);
        if (failure != null) rethrow(failure);
    }

    /** Java resource scopes preserve the primary error if a transport tail fails. */
    private final class DeferredTailScope implements AutoCloseable {
        @Override public void close() { drainDeferredTails(); }
    }

    private static void rethrow(Throwable failure) {
        if (failure instanceof RuntimeException) throw (RuntimeException) failure;
        if (failure instanceof Error) throw (Error) failure;
        throw new AssertionError(failure);
    }
}
