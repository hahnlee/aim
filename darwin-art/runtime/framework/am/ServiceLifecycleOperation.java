package dev.darwinart.runtime.am;

import android.app.IApplicationThread;
import android.app.ServiceStartArgs;
import android.content.Intent;
import android.content.pm.ParceledListSlice;
import android.content.pm.ServiceInfo;
import android.content.res.CompatibilityInfo;
import android.os.Binder;
import android.os.IBinder;
import android.os.RemoteException;
import java.util.concurrent.atomic.AtomicBoolean;

/** One reserved Android service callback, pinned to its original process owner. */
final class ServiceLifecycleOperation {
    enum Kind { CREATE, BIND, ARGS, UNBIND, STOP }

    // Android 16 ActivityManager.PROCESS_STATE_SERVICE (hidden in the SDK jar).
    private static final int PROCESS_STATE_SERVICE = 10;
    final ServiceRecord service;
    final ServiceRecord.LifecycleLane lane;
    final int ownerPid;
    final long ownerSequence;
    final IBinder ownerThread;
    final IntentBindRecord binding;
    private final IApplicationThread thread;
    private final IBinder token;
    private final ServiceInfo info;
    private final Intent intent;
    final boolean rebind;
    private final long bindSequence;
    final Kind kind;
    /** ARGS only: start requests handed to ActivityThread.handleServiceArgs. */
    final java.util.List<ServiceStartArgs> startArgs;
    private final AtomicBoolean dispatched = new AtomicBoolean();

    ServiceLifecycleOperation(ServiceRecord record, IntentBindRecord binding, Kind type) {
        this(record, binding, type, null);
    }

    ServiceLifecycleOperation(ServiceRecord record, IntentBindRecord binding, Kind type,
            java.util.List<ServiceStartArgs> args) {
        if (record.applicationThread == null || record.lifecycleLane == null
                || record.lifecycleLane.closed) {
            throw new IllegalStateException("Service operation has no process owner");
        }
        service = record;
        this.binding = binding;
        lane = record.lifecycleLane;
        thread = record.applicationThread;
        ownerThread = lane.ownerThread;
        ownerPid = lane.ownerPid;
        ownerSequence = lane.ownerStartSequence;
        // Only the exact owner lane's capability crosses into app Binder.  The
        // ServiceRecord canonical identity remains system-server state.
        token = lane.callbackToken;
        info = record.serviceInfo;
        intent = binding == null ? null : new Intent(binding.intent);
        rebind = binding != null && binding.doRebind;
        bindSequence = binding == null ? 0 : binding.bindSequence;
        kind = type;
        if ((type == Kind.ARGS) != (args != null && !args.isEmpty())) {
            throw new IllegalArgumentException("Service start arguments belong only to ARGS");
        }
        startArgs = args;
    }

    /** State-only exact-owner check; caller owns the controller monitor. */
    boolean matchesOwner() {
        return service.lifecycleLane == lane && !lane.closed
                && service.ownerPid == ownerPid && service.ownerStartSequence == ownerSequence
                && service.applicationThread != null
                && service.applicationThread.asBinder().equals(ownerThread);
    }

    /** One-shot real IApplicationThread transport; no internal owner lock or policy. */
    void dispatch() throws RemoteException {
        if (!dispatched.compareAndSet(false, true)) {
            throw new IllegalStateException("Service operation already dispatched");
        }
        long identity = Binder.clearCallingIdentity();
        try {
            switch (kind) {
                case CREATE:
                    thread.scheduleCreateService(token, info,
                            CompatibilityInfo.DEFAULT_COMPATIBILITY_INFO, PROCESS_STATE_SERVICE);
                    break;
                case BIND:
                    thread.scheduleBindService(token, intent, rebind,
                            PROCESS_STATE_SERVICE, bindSequence);
                    break;
                case ARGS:
                    thread.scheduleServiceArgs(token, new ParceledListSlice<>(startArgs));
                    break;
                case UNBIND:
                    thread.scheduleUnbindService(token, intent);
                    break;
                case STOP:
                    thread.scheduleStopService(token);
                    break;
            }
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }
}
