package dev.aim.server;

import dev.aim.server.IWindowShellListener;

/**
 * The device's window shell, the task organizer and transition player of
 * the lightweight shell (docs/task-organizer.md), as `aim.window_shell`.
 * It answers only the system uid.
 */
interface IWindowShell {
    /**
     * Window mode: new tasks on the default display become freeform, and
     * `listener` hears of every organized task, then of each change, until
     * it dies.
     */
    void attach(IWindowShellListener listener);

    /**
     * Moves or resizes task `taskId` in picture-in-picture, as the user
     * moved its window: WindowManager's resizeTask leaves pinned tasks to
     * the shell.
     */
    void setPipBounds(int taskId, int left, int top, int right, int bottom);
}
