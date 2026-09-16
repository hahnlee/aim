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
    final IBinder token = new Binder();
    final HashMap<android.content.Intent.FilterComparison, IntentBindRecord> bindings =
            new HashMap<>();
    IApplicationThread applicationThread;
    int ownerPid;
    long ownerStartSequence = -1;
    boolean processRequested;
    boolean createScheduled;
    boolean retiring;
    boolean stopScheduled;
    int executingCallbacks;

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
