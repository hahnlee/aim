package dev.darwinart.runtime.am;

import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteException;
import java.util.ArrayList;
import java.util.IdentityHashMap;

/** Captured publication delivery; AMS owns admission, never transport under its lock. */
final class ServiceNotificationController {
    interface AdmissionPort {
        boolean currentLocked(ConnectionRecord connection, IBinder.DeathRecipient recipient);
    }
    interface FailurePort {
        void removeLocked(ConnectionRecord connection) throws RemoteException;
    }

    /** Immutable publication identity, distinct from genuine inbound BIND completion. */
    static final class Publication {
        final IntentBindRecord binding;
        final ServiceRecord service;
        final IBinder serviceToken;
        final ServiceRecord.LifecycleLane lane;
        final IBinder binder;
        final long stamp;
        Publication(IntentBindRecord record, IBinder published, long sequence) {
            binding = record;
            service = record.service;
            serviceToken = service.token;
            lane = service.lifecycleLane;
            binder = published; // null is a genuine Android null binding.
            stamp = sequence;
        }
    }

    static final class State {
        Publication desired;
        final IdentityHashMap<ConnectionRecord, Publication> delivered = new IdentityHashMap<>();
        final IdentityHashMap<ConnectionRecord, NotificationOperation> inFlight =
                new IdentityHashMap<>();
    }

    private final Object monitor;
    private final ServiceConnectionIndex index;
    private final AdmissionPort admission;
    private final FailurePort failurePort;
    private long nextStamp = 1;

    ServiceNotificationController(Object ownerMonitor, ServiceConnectionIndex connections,
            AdmissionPort policyAdmission, FailurePort policyFailure) {
        monitor = ownerMonitor;
        index = connections;
        admission = policyAdmission;
        failurePort = policyFailure;
    }

    void publishLocked(IntentBindRecord binding, IBinder published) {
        if (binding == null || binding.publicationReceived) return;
        if (nextStamp == Long.MAX_VALUE) throw new IllegalStateException("Publication stamp exhausted");
        Publication publication = new Publication(binding, published, nextStamp);
        binding.notification.desired = publication;
        binding.publishedBinder = published;
        binding.publicationReceived = true;
        nextStamp++;
    }

    private boolean currentPublicationLocked(Publication publication) {
        if (publication == null) return false;
        ServiceRecord service = publication.service;
        return publication.binding.publicationReceived
                && publication.binding.notification.desired == publication
                && service.token == publication.serviceToken
                && publication.lane != null && service.lifecycleLane == publication.lane
                && !publication.lane.closed && service.applicationThread != null
                && service.applicationThread.asBinder().equals(publication.lane.ownerThread);
    }

    private NotificationOperation claimLocked(ConnectionRecord connection) {
        if (connection == null) return null;
        State state = connection.binding.notification;
        Publication publication = state.desired;
        NotificationOperation old = state.inFlight.get(connection);
        IBinder.DeathRecipient recipient = index.death(connection.connectionBinder);
        if (!currentPublicationLocked(publication)
                || state.delivered.get(connection) == publication
                || old != null
                || !admission.currentLocked(connection, recipient)) return null;
        NotificationOperation operation = new NotificationOperation(
                state, connection, publication, recipient);
        // One transport lane per exact connection preserves publication delivery order.
        state.inFlight.put(connection, operation);
        return operation;
    }

    private boolean currentLocked(NotificationOperation operation) {
        return operation.state.inFlight.get(operation.connection) == operation
                && currentPublicationLocked(operation.publication)
                && admission.currentLocked(operation.connection, operation.recipient);
    }

    private void settleLocked(NotificationOperation operation, boolean delivered) {
        if (operation.state.inFlight.get(operation.connection) != operation) return;
        operation.state.inFlight.remove(operation.connection);
        if (delivered) operation.state.delivered.put(operation.connection, operation.publication);
    }

    void detachLocked(ConnectionRecord connection) {
        State state = connection.binding.notification;
        state.delivered.remove(connection);
        // An admitted transport remains real until its exact tail returns.
    }

    void invalidateLocked(IntentBindRecord binding) {
        binding.notification.desired = null;
        binding.notification.delivered.clear();
        // Retain active claims for ordering and honest transport lifetime.
    }

    /** Only the exact still-current record can be detached by its captured failure. */
    private void failedLocked(NotificationOperation operation, Throwable failure) {
        boolean current = currentLocked(operation);
        settleLocked(operation, false);
        if (failure instanceof RemoteException && current) {
            try {
                failurePort.removeLocked(operation.connection);
            } catch (RemoteException | RuntimeException | Error cleanup) {
                if (cleanup != failure) failure.addSuppressed(cleanup);
            }
        }
    }

    Throwable dispatchOne(ConnectionRecord connection) {
        Throwable firstFailure = null;
        for (;;) {
            NotificationOperation operation;
            synchronized (monitor) { operation = claimLocked(connection); }
            if (operation == null) return firstFailure;
            try {
                operation.dispatch();
                synchronized (monitor) { settleLocked(operation, currentLocked(operation)); }
            } catch (RemoteException | RuntimeException | Error failure) {
                synchronized (monitor) { failedLocked(operation, failure); }
                if (firstFailure == null) firstFailure = failure;
                else if (firstFailure != failure) firstFailure.addSuppressed(failure);
                synchronized (monitor) {
                    if (operation.state.desired == operation.publication) return firstFailure;
                }
            }
            // A new publication blocked on this exact transport gets a genuine
            // completion wake, not an executor, timeout or synthetic callback.
        }
    }

    private void wakeLatestAfterFailure(NotificationOperation operation, Throwable failure) {
        boolean latest;
        synchronized (monitor) { latest = operation.state.desired != operation.publication; }
        if (!latest) return;
        try {
            Throwable next = dispatchOne(operation.connection);
            if (next != null && next != failure) failure.addSuppressed(next);
        } catch (RuntimeException | Error next) {
            if (next != failure) failure.addSuppressed(next);
        }
    }

    void dispatchBinding(IntentBindRecord binding) throws RemoteException {
        for (;;) {
            NotificationOperation operation = null;
            synchronized (monitor) {
                for (ConnectionRecord connection : new ArrayList<>(binding.connections)) {
                    operation = claimLocked(connection);
                    if (operation != null) break;
                }
            }
            if (operation == null) return;
            try {
                operation.dispatch();
            } catch (RemoteException deadClient) {
                synchronized (monitor) { failedLocked(operation, deadClient); }
                if (deadClient.getSuppressed().length != 0) {
                    wakeLatestAfterFailure(operation, deadClient);
                    throw deadClient;
                }
                continue;
            } catch (RuntimeException | Error failure) {
                synchronized (monitor) { failedLocked(operation, failure); }
                wakeLatestAfterFailure(operation, failure);
                throw failure;
            }
            synchronized (monitor) { settleLocked(operation, currentLocked(operation)); }
        }
    }

    static final class NotificationOperation {
        final State state;
        final ConnectionRecord connection;
        final Publication publication;
        final IBinder.DeathRecipient recipient;
        private boolean dispatched;
        NotificationOperation(State deliveryState, ConnectionRecord record,
                Publication capturedPublication, IBinder.DeathRecipient capturedRecipient) {
            state = deliveryState;
            connection = record;
            publication = capturedPublication;
            recipient = capturedRecipient;
        }
        void dispatch() throws RemoteException {
            if (dispatched) throw new IllegalStateException("Notification already dispatched");
            dispatched = true;
            long identity = Binder.clearCallingIdentity();
            try {
                connection.connection.connected(publication.service.component, publication.binder, false);
            } finally {
                Binder.restoreCallingIdentity(identity);
            }
        }
    }
}
