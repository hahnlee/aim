package dev.darwinart.runtime.am;

import android.content.Intent;
import android.os.RemoteException;
import java.util.ArrayDeque;

/** Android service lifecycle policy and exact-owner synchronous transport admission. */
final class ServiceLifecycleController {
    private final ActiveServices owner;
    private final ArrayDeque<ServiceRecord.LifecycleLane> ready = new ArrayDeque<>();

    ServiceLifecycleController(ActiveServices services) { owner = services; }

    /** No pre-reserved operation queue: a later claim reads the current state. */
    void markDirtyLocked(ServiceRecord service) {
        cleanupLocked(service);
        ServiceRecord.LifecycleLane lane = service.lifecycleLane;
        if (lane == null || lane.closed || lane.transportSealed
                || lane.ready || lane.inFlight != null) return;
        lane.ready = true;
        ready.addLast(lane);
    }

    void closeLocked(ServiceRecord.LifecycleLane lane) {
        lane.closed = true;
        // Its in-flight caller still pins this lane; its tail cannot clear a replacement.
    }

    /** Legacy lock-held Binder reentry leaves work for the outermost entry. */
    boolean drain() {
        if (Thread.holdsLock(owner)) return false;
        boolean progressed = false;
        for (;;) {
            ServiceLifecycleOperation operation;
            synchronized (owner) { operation = claimNextLocked(); }
            if (operation == null) return progressed;
            progressed = true;
            Throwable failure = null;
            try {
                if (Thread.holdsLock(owner)) throw new IllegalStateException("Locked lifecycle IPC");
                operation.dispatch();
            } catch (RemoteException | RuntimeException | Error error) {
                failure = error;
            }
            boolean terminalOwner = false;
            if (failure instanceof RemoteException) {
                // Android's FAILED_TRANSACTION heuristic can also raise DeadObjectException.
                // Only the captured Binder's sticky liveness truth proves terminal ownership.
                try { terminalOwner = !operation.ownerThread.isBinderAlive(); }
                catch (RuntimeException | Error queryFailure) {
                    if (queryFailure != failure) failure.addSuppressed(queryFailure);
                }
            }
            synchronized (owner) {
                ServiceRecord.LifecycleLane lane = operation.lane;
                if (lane.inFlight == operation) {
                    lane.inFlight = null;
                    if (operation.matchesOwner() && owner.lifecycleCanonicalLocked(operation.service)) {
                        if (failure instanceof RemoteException) {
                            owner.lifecycleTransportFailedLocked(operation, failure, terminalOwner);
                        } else if (failure != null) {
                            // An unchecked transport failure has ambiguous enqueue
                            // outcome, not proof of process death or completion.
                            lane.transportSealed = true;
                            lane.transportFailure = failure;
                        }
                        markDirtyLocked(operation.service);
                    }
                }
            }
            if (failure instanceof RemoteException) {
                System.err.println("Service lifecycle transport failure: component="
                        + operation.service.component.flattenToString() + " owner="
                        + operation.ownerPid + "/" + operation.ownerSequence
                        + " terminal=" + terminalOwner);
                failure.printStackTrace(System.err);
            }
            if (failure instanceof RuntimeException) throw (RuntimeException) failure;
            if (failure instanceof Error) throw (Error) failure;
        }
    }

    private ServiceLifecycleOperation claimNextLocked() {
        while (!ready.isEmpty()) {
            ServiceRecord.LifecycleLane lane = ready.removeFirst();
            lane.ready = false;
            ServiceRecord service = lane.service;
            if (lane.closed || lane.transportSealed || lane.inFlight != null || service.lifecycleLane != lane
                    || !owner.lifecycleCanonicalLocked(service)) continue;
            ServiceLifecycleOperation operation = planNextLocked(service);
            if (operation != null) {
                lane.inFlight = operation;
                return operation;
            }
            cleanupLocked(service);
        }
        return null;
    }

    private ServiceLifecycleOperation planNextLocked(ServiceRecord service) {
        if (!service.createScheduled && !service.retiring && hasDemand(service)) {
            return reserveLocked(service, null, ServiceLifecycleOperation.Kind.CREATE);
        }
        // A last-client edge survives demand returning before dispatch.
        for (IntentBindRecord binding : service.bindings.values()) {
            if (binding.unbindRequested && binding.hasBound && !binding.unbindScheduled) {
                return reserveLocked(service, binding, ServiceLifecycleOperation.Kind.UNBIND);
            }
        }
        if (service.createScheduled && !service.retiring) {
            for (IntentBindRecord binding : service.bindings.values()) {
                if (binding.hasAdmittedConnections() && !binding.unbindScheduled
                        && !binding.bindCallbackPending && !binding.rebindCallbackPending
                        && (!binding.bindScheduled || binding.doRebind)) {
                    return reserveLocked(service, binding, ServiceLifecycleOperation.Kind.BIND);
                }
            }
        }
        if (service.retiring && service.pendingBinds == 0 && service.createScheduled && !service.stopScheduled
                && service.executingCallbacks == 0 && !hasPendingUnbind(service)) {
            return reserveLocked(service, null, ServiceLifecycleOperation.Kind.STOP);
        }
        return null;
    }

    private ServiceLifecycleOperation reserveLocked(ServiceRecord service,
            IntentBindRecord binding, ServiceLifecycleOperation.Kind kind) {
        ServiceLifecycleOperation operation = new ServiceLifecycleOperation(service, binding, kind);
        switch (kind) {
            case CREATE:
                service.createScheduled = true;
                service.createCallbackPending = true;
                break;
            case BIND:
                binding.requested = true;
                binding.bindScheduled = true;
                binding.hasBound = true;
                binding.bindCallbackPending = !operation.rebind;
                binding.rebindCallbackPending = operation.rebind;
                binding.doRebind = false;
                break;
            case UNBIND:
                binding.unbindRequested = false;
                binding.unbindScheduled = true;
                binding.hasBound = false;
                break;
            case STOP:
                service.stopScheduled = true;
                service.stopCallbackPending = true;
                owner.removeLifecycleCatalogKeyLocked(service);
                break;
        }
        service.executingCallbacks++;
        return operation;
    }

    void publishedLocked(ServiceRecord service, IntentBindRecord binding) {
        if (binding == null || !binding.bindCallbackPending) return;
        binding.bindCallbackPending = false;
        completeLocked(service);
    }

    void unbindFinishedLocked(ServiceRecord service, IntentBindRecord binding, boolean rebind) {
        if (binding == null || !binding.unbindScheduled) return;
        binding.unbindScheduled = false;
        binding.doRebind = rebind;
        completeLocked(service);
    }

    /** Android 16 ActivityThread's actual completion kinds, not inferred binding identity. */
    void doneLocked(ServiceRecord service, int type, Intent intent) {
        IntentBindRecord binding = intent == null ? null
                : service.bindings.get(new Intent.FilterComparison(intent));
        switch (type) {
            case 0: // CREATE / anonymous
                if (!service.createCallbackPending) return;
                service.createCallbackPending = false;
                break;
            case 2: // STOP
                if (!service.stopCallbackPending) return;
                service.stopCallbackPending = false;
                break;
            case 3: // REBIND
                if (binding == null || !binding.rebindCallbackPending) return;
                binding.rebindCallbackPending = false;
                break;
            case 4: // UNBIND(false)
                unbindFinishedLocked(service, binding, false);
                return;
            default:
                throw new IllegalArgumentException("Unsupported service completion kind: " + type);
        }
        completeLocked(service);
    }

    private void completeLocked(ServiceRecord service) {
        if (service.executingCallbacks > 0) service.executingCallbacks--;
        markDirtyLocked(service);
    }

    void cleanupLocked(ServiceRecord service) {
        ServiceRecord.LifecycleLane lane = service.lifecycleLane;
        if (service.retiring && service.pendingBinds == 0 && service.executingCallbacks == 0
                && (lane == null || lane.inFlight == null)
                && (!service.createScheduled || service.stopScheduled)) {
            owner.removeLifecycleServiceLocked(service);
        }
    }

    private static boolean hasDemand(ServiceRecord service) {
        for (IntentBindRecord binding : service.bindings.values()) {
            if (binding.hasAdmittedConnections()) return true;
        }
        return false;
    }

    private static boolean hasPendingUnbind(ServiceRecord service) {
        for (IntentBindRecord binding : service.bindings.values()) {
            if (binding.unbindRequested || binding.unbindScheduled) return true;
        }
        return false;
    }
}
