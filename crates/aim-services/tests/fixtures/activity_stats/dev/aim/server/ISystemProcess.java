package dev.aim.server;
interface ISystemProcess {
    IActivityStatsHost getActivityStatsHost(IActivityStatsSnapshot collector);
    IAccessibilityWindowHost getAccessibilityWindowHost();
}
