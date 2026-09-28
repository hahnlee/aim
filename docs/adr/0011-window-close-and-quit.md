# ADR 0011: Window close backgrounds, Quit removes the task

Status: accepted (#15)

## Context

Each Android application process owns one macOS window (its desktop root,
ADR 0008) and runs as its own macOS application with a Dock icon. Closing
that window used to hide it and leave the Activities resumed, so a game kept
its loop and audio running with no window. Cmd+Q did nothing.

Android has two user gestures with the same intent: Home sends the task to
the background, where its Activities pause and stop but the process and its
services live on. Removing the task from Recents destroys its Activities and
usually ends the process.

## Decision

- **Close (red button) and Minimize are Home.** The window is ordered out, not
  closed: the root, its surfaces and its task survive. The host reports the
  root as hidden and ActivityTask moves the task to the back: the visible
  Activities lose app visibility and pause and stop. The process and its
  services keep running, so a download service keeps downloading.
- **Back on the task's root is Home too.** As ActivityClientController.
  onBackPressed does for a task root started from the launcher, the system
  server moves the task to the back: it asks the host (HOST_HIDE on the root
  geometry receiver) to close the root as its close button would, and the
  host's hidden report stops the task. Back anywhere else finishes that
  Activity.
- **Dock reopen and Deminimize return to the app.** The window is shown again,
  the Activities regain app visibility and the top one resumes through the
  restart path (`onRestart` → `onStart` → `onResume`). It is not recreated.
- **Quit (Cmd+Q, the app menu, the Dock) is removal from Recents.** The host
  reports quit and ActivityTask destroys every Activity of the task, top first
  (the executor pauses and stops each on the way). The system server then
  ends the process once the Activities are gone, or after five seconds.
  AppKit's own termination is the fallback if the system server does not act
  within five seconds.
- **Foreground services do not keep a quit app alive.** On Android, removing a
  task may leave a foreground service running. On macOS, Quit means the whole
  application stops, and a service with no window would be invisible to the
  user and impossible to quit again. Quit therefore ends the process
  regardless of its services.

The app process pumps AppKit itself and never runs `-finishLaunching`, so it
registers the reopen (`rapp`) and quit Apple Event handlers explicitly. The
Dock and `Quit` from other applications reach the same delegate paths as the
menu.

## Consequences

Closing a window no longer means losing the app's state, which matches both
Android's Home and the macOS convention for single-window apps that stay in
the Dock. Launching a package whose application process is running brings
that process back through the same reopen event (as LaunchServices does for a
running macOS app) instead of starting a second process for the package.
