package dev.darwinart.runtime.am;

import android.content.Intent;
import android.os.IBinder;
import java.util.ArrayList;

/** Binding state for one Intent filter within a ServiceRecord. */
final class IntentBindRecord {
    final ServiceRecord service;
    final Intent intent;
    final String resolvedType;
    final long bindSequence;
    final ArrayList<ConnectionRecord> connections = new ArrayList<>();
    final ServiceNotificationController.State notification =
            new ServiceNotificationController.State();
    int pendingBinds;
    boolean requested;
    boolean bindScheduled;
    boolean unbindRequested;
    boolean unbindScheduled;
    boolean publicationReceived;
    boolean hasBound;
    boolean doRebind;
    boolean bindCallbackPending;
    boolean rebindCallbackPending;
    IBinder publishedBinder;

    boolean hasAdmittedConnections() {
        for (ConnectionRecord connection : connections) {
            if (connection.admitted) return true;
        }
        return false;
    }

    IntentBindRecord(ServiceRecord owner, Intent requestedIntent, String type, long sequence) {
        service = owner;
        intent = new Intent(requestedIntent);
        resolvedType = type;
        bindSequence = sequence;
    }
}
