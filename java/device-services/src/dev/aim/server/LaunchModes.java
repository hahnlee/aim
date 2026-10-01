package dev.aim.server;

import android.app.ActivityOptions;
import android.app.WindowConfiguration;
import android.content.ComponentName;
import android.content.pm.ActivityInfo;
import android.os.SystemClock;

import com.android.server.wm.ActivityInterceptorCallback;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * The windowing mode each activity start asked for, for the window shell
 * (docs/task-organizer.md, "Explicit windowing modes"). On the shell's
 * fullscreen display area WindowManager resolves a start without a mode and
 * one that asked for fullscreen alike, and the shell makes only the former
 * freeform, as a freeform display area does. It observes starts and never
 * intercepts one: it always answers null.
 */
final class LaunchModes implements ActivityInterceptorCallback {
    /** How long a start's mode waits for its task's transition request. */
    private static final long TTL_MS = 10_000;

    private record Launch(int mode, long time) {}

    /** The last start of each component that asked for a mode. */
    private final Map<ComponentName, Launch> mLaunches = new ConcurrentHashMap<>();

    @Override
    public ActivityInterceptResult onInterceptActivityLaunch(ActivityInterceptorInfo info) {
        ActivityInfo activity = info.getActivityInfo();
        if (activity == null) {
            return null;
        }
        long now = SystemClock.uptimeMillis();
        mLaunches.values().removeIf(l -> now - l.time() > TTL_MS);
        ActivityOptions options = info.getCheckedOptions();
        int mode = options != null ? options.getLaunchWindowingMode()
                : WindowConfiguration.WINDOWING_MODE_UNDEFINED;
        ComponentName component = new ComponentName(activity.packageName, activity.name);
        if (mode != WindowConfiguration.WINDOWING_MODE_UNDEFINED) {
            mLaunches.put(component, new Launch(mode, now));
        } else {
            mLaunches.remove(component);
        }
        return null;
    }

    /** The windowing mode the latest start of `component` asked for. */
    int peek(ComponentName component) {
        return mode(mLaunches.get(component));
    }

    /** The windowing mode the latest start of `component` asked for, once. */
    int take(ComponentName component) {
        return mode(mLaunches.remove(component));
    }

    private static int mode(Launch launch) {
        return launch != null && SystemClock.uptimeMillis() - launch.time() <= TTL_MS
                ? launch.mode() : WindowConfiguration.WINDOWING_MODE_UNDEFINED;
    }
}
