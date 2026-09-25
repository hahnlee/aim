package dev.darwinart.runtime.am;

import android.app.ServiceStartArgs;
import android.content.Intent;
import java.util.ArrayList;

/**
 * Started-service policy of Android's ActiveServices: start ids, stopSelf(startId)
 * matching, onStartCommand results and restart-after-death demand.
 *
 * <p>State lives on {@link ServiceRecord}; every method runs with the
 * ActiveServices monitor held. Lifecycle transport (create, args, stop) stays
 * with {@link ServiceLifecycleController}.</p>
 */
final class StartedServiceRequests {
    // Android 16 Service.onStartCommand results.
    static final int START_STICKY_COMPATIBILITY = 0;
    static final int START_STICKY = 1;
    static final int START_NOT_STICKY = 2;
    static final int START_REDELIVER_INTENT = 3;
    static final int START_TASK_REMOVED_COMPLETE = 1000;
    // Android 16 Service.STOP_FOREGROUND_REMOVE / DETACH.
    static final int STOP_FOREGROUND_REMOVE = 1;
    // ActivityManagerConstants defaults for service restart policy.
    static final long SERVICE_RESTART_DURATION_MILLIS = 1_000;
    static final int SERVICE_RESTART_DURATION_FACTOR = 4;
    static final long SERVICE_RESET_RUN_DURATION_MILLIS = 60_000;
    static final long SERVICE_MAX_RESTART_DELAY_MILLIS = 60_000;
    static final int MAX_CRASH_RETRY = 16;

    private StartedServiceRequests() {}

    static boolean hasDemand(ServiceRecord service) {
        return service.startRequested;
    }

    /** Context.startService: records one start request and returns its id. */
    static int start(ServiceRecord service, Intent intent) {
        service.lastStartUptimeMillis = android.os.SystemClock.uptimeMillis();
        if (service.lastStartId == Integer.MAX_VALUE) {
            throw new IllegalStateException("Service start id exhausted");
        }
        service.startRequested = true;
        service.stopIfKilled = false;
        int id = ++service.lastStartId;
        service.pendingStarts.addLast(new ServiceRecord.StartItem(id, intent));
        return id;
    }

    /** Context.stopService: true when a started state was cleared. */
    static boolean stop(ServiceRecord service) {
        if (!service.startRequested) return false;
        clear(service);
        return true;
    }

    /**
     * Service.stopSelfResult(startId): a negative id stops unconditionally;
     * otherwise only the most recent start may stop the service, so a newer
     * pending start is never lost to an older completion.
     */
    static boolean stopSelf(ServiceRecord service, int startId) {
        if (startId >= 0 && startId != service.lastStartId) return false;
        clear(service);
        return true;
    }

    /** Takes queued requests for one scheduleServiceArgs batch. */
    static ArrayList<ServiceStartArgs> takePending(ServiceRecord service) {
        ArrayList<ServiceStartArgs> args = new ArrayList<>(service.pendingStarts.size());
        while (!service.pendingStarts.isEmpty()) {
            ServiceRecord.StartItem item = service.pendingStarts.removeFirst();
            args.add(new ServiceStartArgs(false, item.id, 0,
                    item.intent == null ? null : new Intent(item.intent)));
        }
        service.deliveredStarts += args.size();
        return args;
    }

    /** SERVICE_DONE_EXECUTING_START for one delivered start; false if none was pending. */
    static boolean done(ServiceRecord service, int startId, int result) {
        if (result == START_TASK_REMOVED_COMPLETE) return false;
        if (service.deliveredStarts <= 0) return false;
        service.deliveredStarts--;
        switch (result) {
            case START_STICKY_COMPATIBILITY:
            case START_STICKY:
            case START_REDELIVER_INTENT:
                service.stopIfKilled = false;
                break;
            case START_NOT_STICKY:
                if (startId == service.lastStartId) service.stopIfKilled = true;
                break;
            default:
                throw new IllegalArgumentException("Unknown service start result " + result);
        }
        return true;
    }

    /**
     * Owner process death: a sticky started service restarts with a null
     * intent in the replacement owner; START_NOT_STICKY drops the demand.
     */
    static void ownerGone(ServiceRecord service) {
        // Dying while a start callback was executing counts as a service crash.
        if (service.deliveredStarts > 0 || service.createCallbackPending) service.crashCount++;
        service.deliveredStarts = 0;
        service.foreground = false;
        service.foregroundId = 0;
        service.foregroundServiceType = 0;
        if (!service.startRequested) return;
        if (service.crashCount >= MAX_CRASH_RETRY
                || (service.stopIfKilled && service.pendingStarts.isEmpty())) {
            clear(service);
        } else if (service.pendingStarts.isEmpty()) {
            start(service, null);
        }
    }

    /**
     * ActiveServices.scheduleServiceRestartLocked delay: the first restart
     * waits one duration, repeated restarts grow by the factor up to the cap,
     * and a service that ran for the reset duration starts over.
     */
    static long nextRestartDelay(ServiceRecord service, long now) {
        if (service.restartDelayMillis == 0
                || now - service.lastStartUptimeMillis > SERVICE_RESET_RUN_DURATION_MILLIS) {
            service.restartDelayMillis = SERVICE_RESTART_DURATION_MILLIS;
        } else {
            service.restartDelayMillis = Math.min(SERVICE_MAX_RESTART_DELAY_MILLIS,
                    service.restartDelayMillis * SERVICE_RESTART_DURATION_FACTOR);
        }
        return service.restartDelayMillis;
    }

    /** Service.startForeground / stopForeground. */
    static void setForeground(ServiceRecord service, int id, boolean hasNotification, int flags,
            int foregroundServiceType) {
        if (hasNotification) {
            if (id == 0) throw new IllegalArgumentException("Foreground notification id is 0");
            service.foreground = true;
            service.foregroundId = id;
            service.foregroundServiceType = foregroundServiceType;
        } else if (service.foreground) {
            service.foreground = false;
            if ((flags & STOP_FOREGROUND_REMOVE) != 0) service.foregroundId = 0;
            service.foregroundServiceType = 0;
        }
    }

    private static void clear(ServiceRecord service) {
        service.startRequested = false;
        service.stopIfKilled = false;
        service.pendingStarts.clear();
        service.foreground = false;
        service.foregroundId = 0;
        service.foregroundServiceType = 0;
    }
}
