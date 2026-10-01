package dev.aim.server;

/** aim-windows' side of the window shell (IWindowShell). */
oneway interface IWindowShellListener {
    /**
     * An organized task, when it appears and whenever its activity type
     * (WindowConfiguration.ACTIVITY_TYPE_*), its top activity's manifest
     * orientation (ActivityInfo.screenOrientation) or its windowing mode
     * (WindowConfiguration.WINDOWING_MODE_*) changes.
     */
    void onTaskChanged(int taskId, int activityType, int orientation, int windowingMode);

    /**
     * A transition an organized task took part in finished: its surface is
     * at its bounds and shows what the task has drawn (its starting window
     * or its app).
     */
    void onTaskPlaced(int taskId);
}
