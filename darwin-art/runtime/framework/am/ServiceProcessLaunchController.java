package dev.darwinart.runtime.am;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.IdentityHashMap;

/** Android service demand policy; opaque process resources belong to the transport. */
final class ServiceProcessLaunchController {
    interface StatePort {
        ArrayList<ServiceRecord> servicesLocked();
        long nextSequenceLocked();
        void launchFailedLocked(ConnectionRecord initiating, long ownerRevision, Throwable failure);
    }

    private static final class ProcessKey {
        final String packageName;
        final String processName;
        final int uid;
        final boolean isolated;

        ProcessKey(ServiceRecord service) {
            packageName = service.component.getPackageName();
            processName = service.processName;
            uid = service.uid;
            isolated = service.isolated;
        }

        @Override public int hashCode() {
            return 31 * (31 * packageName.hashCode() + processName.hashCode()) + uid
                    + (isolated ? 1 : 0);
        }

        @Override public boolean equals(Object other) {
            if (!(other instanceof ProcessKey)) return false;
            ProcessKey key = (ProcessKey) other;
            return uid == key.uid && isolated == key.isolated
                    && packageName.equals(key.packageName) && processName.equals(key.processName);
        }
    }

    private enum Phase { PREPARING, PREPARED, ACTIVATING, ACTIVE, FAILED, FINISHED }

    private static final class Attempt {
        final ProcessKey key;
        final long sequence;
        ConnectionRecord initiating;
        final long initiatingOwnerRevision;
        Phase phase = Phase.PREPARING;
        ProcessLaunchTransport.PreparedLaunch prepared;
        boolean transportInFlight;
        boolean retirementRequested;
        boolean retirementInFlight;
        boolean retirementSettled;
        boolean supersededDemand;
        long failedRetirementEpoch;

        Attempt(ProcessKey process, long startSequence, ConnectionRecord connection) {
            key = process;
            sequence = startSequence;
            initiating = connection;
            initiatingOwnerRevision = connection == null ? 0 : connection.binding.service.ownerRevision;
        }
    }

    private static final class Demand {
        final ProcessKey key;
        final IdentityHashMap<ServiceRecord, Boolean> members = new IdentityHashMap<>();
        Attempt attempt;
        Demand(ProcessKey process) { key = process; }
    }

    private enum Kind { PREPARE, ACTIVATE, RETIRE }
    private static final class DeathResources {
        final ApplicationProcessRegistry.AttachedApplication gone;
        boolean inFlight;
        long failedEpoch;
        DeathResources(ApplicationProcessRegistry.AttachedApplication snapshot) { gone = snapshot; }
    }
    private static final class Operation {
        final Attempt attempt;
        final Kind kind;
        Operation(Attempt value, Kind operation) { attempt = value; kind = operation; }
    }

    private final Object owner;
    private final StatePort state;
    private final ApplicationProcessRegistry registry;
    private final ProcessLaunchTransport transport;
    private final HashMap<ProcessKey, Demand> demands = new HashMap<>();
    private long drainEpoch;
    private final ArrayList<DeathResources> deaths = new ArrayList<>();

    ServiceProcessLaunchController(Object monitor, StatePort statePort,
            ApplicationProcessRegistry processes, ProcessLaunchTransport launchTransport) {
        owner = monitor;
        state = statePort;
        registry = processes;
        transport = launchTransport;
    }

    void processGoneLocked(ApplicationProcessRegistry.AttachedApplication gone) {
        deaths.add(new DeathResources(gone));
    }

    /** Explicit demand admission, not permission to revive a cancelled attempt. */
    void requestLocked(ServiceRecord service, ConnectionRecord initiating) {
        ProcessKey key = new ProcessKey(service);
        Demand demand = demands.get(key);
        if (demand == null) {
            demand = new Demand(key);
            demands.put(key, demand);
        }
        Attempt previous = demand.attempt;
        if (previous != null && previous.retirementRequested && demanded(service)) {
            previous.supersededDemand = true;
        }
        if (previous != null && previous.phase == Phase.FAILED
                && !previous.transportInFlight && !previous.retirementInFlight
                && previous.retirementSettled
                && (initiating == null || initiating != previous.initiating)) {
            demand.attempt = null;
        }
        if (demand.attempt == null && demanded(service) && service.applicationThread == null
                && !attached(key)) {
            demand.attempt = new Attempt(key, state.nextSequenceLocked(), initiating);
        }
        reconcileLocked();
    }

    private boolean attached(ProcessKey key) {
        return registry.findAttached(key.packageName, key.processName, key.uid) != null;
    }

    private static boolean demanded(ServiceRecord service) {
        if (StartedServiceRequests.hasDemand(service)) return true;
        for (IntentBindRecord binding : service.bindings.values()) {
            if (binding.hasAdmittedConnections()) return true;
        }
        return false;
    }

    private static boolean needsLaunch(Demand demand) {
        for (ServiceRecord member : demand.members.keySet()) {
            if (member.applicationThread == null) return true;
        }
        return false;
    }

    private void reconcileLocked() {
        for (Demand demand : demands.values()) demand.members.clear();
        for (ServiceRecord service : state.servicesLocked()) {
            Demand demand = demands.get(new ProcessKey(service));
            if (demand != null && demanded(service)) demand.members.put(service, Boolean.TRUE);
            service.processRequested = demand != null && demand.attempt != null
                    && demand.attempt.phase != Phase.FAILED && demanded(service)
                    && service.applicationThread == null;
        }
        for (Demand demand : demands.values()) {
            Attempt attempt = demand.attempt;
            if (attempt == null) continue;
            if (demand.members.isEmpty() || attached(demand.key)) attempt.retirementRequested = true;
            if (attempt.retirementRequested && attempt.phase == Phase.PREPARING
                    && !attempt.transportInFlight) attempt.retirementSettled = true;
            if (attempt.phase != Phase.FAILED && attempt.retirementSettled && !attempt.transportInFlight
                    && !attempt.retirementInFlight) attempt.phase = Phase.FINISHED;
            if (attempt.phase == Phase.FINISHED || (attempt.phase == Phase.FAILED
                    && attempt.supersededDemand && !attempt.transportInFlight
                    && !attempt.retirementInFlight && attempt.retirementSettled)) {
                demand.attempt = null;
                if (needsLaunch(demand) && !attached(demand.key)) {
                    demand.attempt = new Attempt(demand.key, state.nextSequenceLocked(), null);
                }
            }
        }
        java.util.Iterator<Demand> iterator = demands.values().iterator();
        while (iterator.hasNext()) {
            Demand demand = iterator.next();
            Attempt attempt = demand.attempt;
            if (demand.members.isEmpty() && (attempt == null
                    || (attempt.phase == Phase.FAILED && attempt.retirementSettled
                        && !attempt.transportInFlight && !attempt.retirementInFlight))) {
                iterator.remove();
            }
        }
    }

    private Operation claimLocked(long epoch) {
        reconcileLocked();
        for (Demand demand : demands.values()) {
            Attempt attempt = demand.attempt;
            if (attempt == null) continue;
            if (attempt.retirementRequested && attempt.prepared != null
                    && !attempt.retirementSettled && !attempt.retirementInFlight
                    && attempt.failedRetirementEpoch < epoch) {
                attempt.retirementInFlight = true;
                return new Operation(attempt, Kind.RETIRE);
            }
            if (attempt.phase == Phase.FAILED) continue;
            if (attempt.transportInFlight || attempt.retirementRequested) {
                // Preparation must return its exact token before retirement can dispatch.
                if (attempt.phase != Phase.PREPARING || attempt.transportInFlight) continue;
            }
            if (attempt.phase == Phase.PREPARING) {
                attempt.transportInFlight = true;
                return new Operation(attempt, Kind.PREPARE);
            }
            if (attempt.phase == Phase.PREPARED && !attempt.retirementRequested) {
                attempt.phase = Phase.ACTIVATING;
                attempt.transportInFlight = true;
                return new Operation(attempt, Kind.ACTIVATE);
            }
        }
        return null;
    }

    /** No global drain gate: unrelated keys and retirement can progress during activation. */
    long beginDrain() {
        synchronized (owner) {
            if (drainEpoch == Long.MAX_VALUE) throw new IllegalStateException("Launch drain epoch exhausted");
            return ++drainEpoch;
        }
    }

    boolean drain(long epoch) {
        if (Thread.holdsLock(owner)) return false;
        boolean progressed = false;
        for (;;) {
            DeathResources death = null;
            synchronized (owner) {
                for (DeathResources value : deaths) {
                    if (!value.inFlight && value.failedEpoch < epoch) {
                        death = value;
                        death.inFlight = true;
                        break;
                    }
                }
            }
            if (death != null) {
                progressed = true;
                Throwable failure = null;
                try { transport.onProcessGone(death.gone); }
                catch (RuntimeException | Error error) { failure = error; }
                synchronized (owner) {
                    death.inFlight = false;
                    if (failure == null) deaths.remove(death);
                    else death.failedEpoch = epoch;
                }
                if (failure instanceof RuntimeException) throw (RuntimeException) failure;
                if (failure instanceof Error) throw (Error) failure;
                continue;
            }
            Operation operation;
            synchronized (owner) { operation = claimLocked(epoch); }
            if (operation == null) return progressed;
            progressed = true;
            Attempt attempt = operation.attempt;
            ProcessLaunchTransport.PreparedLaunch prepared = null;
            ProcessLaunchTransport.ActivationResult result = null;
            Throwable failure = null;
            boolean cleanupSettled = false;
            try {
                if (operation.kind == Kind.PREPARE) {
                    prepared = transport.prepare(attempt.key.packageName, attempt.key.processName,
                            attempt.key.uid, attempt.key.isolated, attempt.sequence);
                    if (prepared == null || prepared.pid() <= 0
                            || prepared.startSequence() != attempt.sequence) {
                        throw new IllegalStateException("Invalid prepared process identity");
                    }
                } else if (operation.kind == Kind.ACTIVATE) {
                    result = transport.activate(attempt.prepared);
                    if (result == null) throw new IllegalStateException("Missing activation disposition");
                } else {
                    transport.retireUnattached(attempt.prepared);
                }
            } catch (RuntimeException | Error error) {
                failure = error;
            }
            if (failure != null && operation.kind != Kind.RETIRE) {
                ProcessLaunchTransport.PreparedLaunch cleanup = operation.kind == Kind.PREPARE
                        ? prepared : attempt.prepared;
                if (cleanup != null) {
                    try {
                        transport.retireUnattached(cleanup);
                        cleanupSettled = true;
                    }
                    catch (RuntimeException | Error error) {
                        if (error != failure) failure.addSuppressed(error);
                    }
                }
                else cleanupSettled = true;
            }
            synchronized (owner) {
                Demand demand = demands.get(attempt.key);
                if (demand != null && demand.attempt == attempt) {
                    if (operation.kind == Kind.RETIRE) {
                        attempt.retirementInFlight = false;
                        attempt.retirementSettled = failure == null;
                        if (failure != null) attempt.failedRetirementEpoch = epoch;
                    } else {
                        attempt.transportInFlight = false;
                        if (failure != null) {
                            attempt.phase = Phase.FAILED;
                            attempt.retirementRequested = true;
                            if (operation.kind == Kind.PREPARE) attempt.prepared = prepared;
                            attempt.retirementSettled = cleanupSettled;
                            if (!cleanupSettled) attempt.failedRetirementEpoch = epoch;
                            try {
                                state.launchFailedLocked(attempt.initiating,
                                        attempt.initiatingOwnerRevision, failure);
                            } catch (RuntimeException | Error cleanup) {
                                if (cleanup != failure) failure.addSuppressed(cleanup);
                            }
                            attempt.initiating = null;
                        } else if (operation.kind == Kind.PREPARE) {
                            attempt.prepared = prepared;
                            attempt.phase = Phase.PREPARED;
                        } else {
                            attempt.phase = Phase.ACTIVE;
                            if (result != ProcessLaunchTransport.ActivationResult.ACTIVE) {
                                attempt.retirementRequested = true;
                            }
                        }
                    }
                    reconcileLocked();
                }
            }
            if (failure instanceof RuntimeException) throw (RuntimeException) failure;
            if (failure instanceof Error) throw (Error) failure;
        }
    }
}
