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
    boolean requested;
    boolean bindScheduled;
    boolean unbindScheduled;
    boolean publicationReceived;
    boolean hasBound;
    boolean doRebind;
    IBinder publishedBinder;

    IntentBindRecord(ServiceRecord owner, Intent requestedIntent, String type, long sequence) {
        service = owner;
        intent = new Intent(requestedIntent);
        resolvedType = type;
        bindSequence = sequence;
    }
}
