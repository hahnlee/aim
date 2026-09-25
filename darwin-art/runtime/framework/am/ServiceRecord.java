package dev.darwinart.runtime.am;

import android.app.IApplicationThread;
import android.content.ComponentName;
import android.content.pm.ServiceInfo;
import android.os.Binder;
import android.os.IBinder;
import java.util.HashMap;

/** System-server state for one manifest service instance. */
final class ServiceRecord {
    final ComponentName component;
    final String instanceName;
    final String processName;
    final int uid;
    final boolean isolated;
    final ServiceInfo serviceInfo;
    /** Stable system-server identity; never crosses the application callback boundary. */
    final IBinder canonicalToken = new Binder();
    /** Current guest-facing capability, rotated for every owner incarnation. */
    IBinder token = canonicalToken;
    final HashMap<android.content.Intent.FilterComparison, IntentBindRecord> bindings =
            new HashMap<>();
    IApplicationThread applicationThread;
    int ownerPid;
    long ownerStartSequence = -1;
    long ownerRevision;
    boolean processRequested;
    int pendingBinds;
    boolean createScheduled;
    boolean retiring;
    boolean stopScheduled;
    int executingCallbacks;
    boolean createCallbackPending;
    boolean stopCallbackPending;

    /** One Context.startService request awaiting ServiceStartArgs delivery. */
    static final class StartItem {
        final int id;
        final android.content.Intent intent;

        StartItem(int startId, android.content.Intent startIntent) {
            id = startId;
            intent = startIntent;
        }
    }

    // Started-service demand, as AOSP ServiceRecord.startRequested/pendingStarts.
    boolean startRequested;
    /** START_NOT_STICKY for the latest delivered start: do not restart after death. */
    boolean stopIfKilled;
    int lastStartId;
    final java.util.ArrayDeque<StartItem> pendingStarts = new java.util.ArrayDeque<>();
    /** Delivered start arguments whose SERVICE_DONE_EXECUTING_START has not arrived. */
    int deliveredStarts;
    boolean foreground;
    int foregroundId;
    int foregroundServiceType;
    // AOSP ServiceRecord restart backoff after owner death.
    int crashCount;
    long restartDelayMillis;
    long lastStartUptimeMillis;
    boolean restartScheduled;
    /** Exact-owner transport admission; accessed only while ActiveServices is held. */
    static final class LifecycleLane {
        final ServiceRecord service;
        final int ownerPid;
        final long ownerStartSequence;
        final IBinder ownerThread;
        final ApplicationProcessRegistry.AttachedApplication attachedOwner;
        /** Capability accepted by the app's service callbacks for this lane only. */
        final IBinder callbackToken = new Binder();
        ServiceLifecycleOperation inFlight;
        boolean ready;
        boolean closed;
        boolean transportSealed;
        Throwable transportFailure;

        LifecycleLane(ServiceRecord record, ApplicationProcessRegistry.AttachedApplication attached) {
            service = record;
            attachedOwner = attached;
            ownerPid = attached.pid;
            ownerStartSequence = attached.startSequence;
            ownerThread = attached.thread;
        }
    }

    LifecycleLane lifecycleLane;

    ServiceRecord(ComponentName name, String instance, String process, int processUid,
            boolean isIsolated, ServiceInfo info) {
        component = name;
        instanceName = instance;
        processName = process;
        uid = processUid;
        isolated = isIsolated;
        serviceInfo = info;
    }
}
