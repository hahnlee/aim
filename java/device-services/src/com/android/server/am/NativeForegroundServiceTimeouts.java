package com.android.server.am;

import android.app.ForegroundServiceStartNotAllowedException;
import android.app.compat.CompatChanges;
import android.content.pm.ServiceInfo;
import android.provider.DeviceConfig;
import java.util.HashMap;

/**
 * ActiveServices' time-limit lookup and tracking (android-16.0.0_r1), separated
 * from its AMS instance. The original ServiceRecord$TimeLimitedFgsInfo owns the
 * accounting; the native service owner owns scheduling and service lifetimes.
 * Copyright (C) The Android Open Source Project, Apache License 2.0.
 */
public final class NativeForegroundServiceTimeouts {
    private final HashMap<Long, ServiceRecord$TimeLimitedFgsInfo> limits = new HashMap<>();

    // ActivityManagerConstants defaults; DeviceConfig retains the original owner.
    public long[] policy(int types, int uid) {
        long media = duration("media_processing_fgs_timeout_duration",
                6 * 60 * 60_000L);
        long data = duration("data_sync_fgs_timeout_duration",
                6 * 60 * 60_000L);
        // ActiveServices.getTimeLimitedFgsType: prefer the most lenient type,
        // keeping mediaProcessing on equal limits.
        int type = 0;
        long limit = 0;
        if (CompatChanges.isChangeEnabled(317799821L, uid)
                && android.app.Flags.introduceNewServiceOntimeoutCallback()) {
            if ((types & ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING) != 0) {
                type = ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING;
                limit = media;
            }
            if ((types & ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC) != 0
                    && (limit == 0 || data > limit)) {
                type = ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC;
                limit = data;
            }
        }
        return new long[] {
                duration("short_fgs_timeout_duration", 3 * 60_000L),
                duration("short_fgs_proc_state_extra_wait_duration", 5_000L),
                duration("short_fgs_anr_extra_wait_duration", 10_000L),
                type, limit,
                duration("fgs_crash_extra_wait_duration", 10_000L),
                android.app.Flags.enableFgsTimeoutCrashBehavior() ? 1 : 0,
                duration("service_start_foreground_anr_delay_ms", 10000),
                duration("service_start_foreground_timeout_ms", 30000)
        };
    }

    private static long duration(String key, long fallback) {
        return Math.max(0, DeviceConfig.getLong("activity_manager", key, fallback));
    }

    private static long key(int uid, int type) {
        return ((long) uid << 32) | (type & 0xffffffffL);
    }

    public synchronized long start(int uid, int type, int previousType, boolean top,
            long lastTop, long uptime, long realtime, long limit) {
        var info = limits.computeIfAbsent(key(uid, type), ignored -> new ServiceRecord$TimeLimitedFgsInfo());
        long exceeded = info.getTimeLimitExceededAt();
        if (info.getFirstFgsStartRealtime() < Math.max(0, realtime - 24 * 60 * 60 * 1000L)
                || top || (exceeded != Long.MIN_VALUE && lastTop > exceeded)) {
            info.reset();
        } else if (exceeded > 0) {
            String message = "Time limit already exhausted for foreground service type "
                    + ServiceInfo.foregroundServiceTypeToLabel(type);
            if (android.app.Flags.enableFgsTimeoutCrashBehavior()) {
                throw new ForegroundServiceStartNotAllowedException(message);
            }
            android.util.Slog.w("AimForegroundService", message);
            info.reset();
        }
        if (previousType != 0) stop(uid, previousType, uptime);
        info.noteFgsFgsStart(uptime);
        return info.getLastFgsStartTime() + Math.max(0, limit - info.getTotalRuntime());
    }

    public synchronized void stop(int uid, int type, long uptime) {
        var info = limits.get(key(uid, type));
        if (info != null) {
            info.updateTotalRuntime(uptime);
            info.decNumParallelServices();
        }
    }

    /** ActiveServices.onFgsTimeout: >0 reschedules; zero delivers onTimeout. */
    public synchronized long timeout(int uid, int type, boolean top, long lastTop,
            long uptime, long limit) {
        if (top) lastTop = uptime;
        if (lastTop != Long.MIN_VALUE && limit > uptime - lastTop) return lastTop + limit;
        var info = limits.get(key(uid, type));
        if (info != null) {
            info.updateTotalRuntime(uptime);
            info.setTimeLimitExceededAt(uptime);
        }
        return 0;
    }

    public synchronized void removeUid(int uid) {
        limits.keySet().removeIf(key -> (int) (key >> 32) == uid);
    }
}
