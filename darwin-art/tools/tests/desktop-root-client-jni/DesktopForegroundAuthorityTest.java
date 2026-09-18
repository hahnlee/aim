package dev.darwinart.runtime.wm;

/** Actual authority/native table, with no replacement production provider. */
public final class DesktopForegroundAuthorityTest {
    public static void run(int pid) {
        DesktopForegroundAuthority.ProcessInstance self = DesktopForegroundAuthority.HOST.capture(pid);
        if (self.pid != pid || self.startSeconds <= 0)
            throw new AssertionError("actual kernel identity missing");
        DesktopForegroundAuthority.HOST.isForeground(self); // live status may be either value
        DesktopForegroundAuthority.ProcessInstance forged =
                new DesktopForegroundAuthority.ProcessInstance(pid, self.startSeconds + 1,
                        self.startMicroseconds);
        if (DesktopForegroundAuthority.HOST.isForeground(forged))
            throw new AssertionError("forged kernel birth granted foreground");
        try {
            DesktopForegroundAuthority.HOST.capture(0);
            throw new AssertionError("invalid PID capture succeeded");
        } catch (IllegalStateException expected) {
            // Real provider failure remains failure, not an invented identity.
        }
    }
}
