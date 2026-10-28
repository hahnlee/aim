package dev.aim.server;

oneway interface IAlarmIndicatorListener {
    void changed(@nullable String packageName);
}
