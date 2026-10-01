package dev.aim.server;

/** aim-windows' side of the window shell (IWindowShell). */
oneway interface IWindowShellListener {
    /**
     * An organized task, when it appears and whenever its activity type
     * (WindowConfiguration.ACTIVITY_TYPE_*) or its top activity's manifest
     * orientation (ActivityInfo.screenOrientation) changes.
     */
    void onTaskChanged(int taskId, int activityType, int orientation);
}
