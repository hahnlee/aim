package dev.aim.server;
import dev.aim.server.IAlarmIndicatorListener;

/** NextAlarmController's source, hosted by the system-uid Java process. */
interface IAlarmIndicator {
    void watch(IAlarmIndicatorListener listener);
}
