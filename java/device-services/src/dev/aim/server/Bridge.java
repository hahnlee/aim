package dev.aim.server;

/** Authentication shared by the original-system-owner leaf Binder bridges. */
public final class Bridge {
    private Bridge() {}
    public static void enforceSystemUid() {
        if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID)
            throw new SecurityException("native package leaf requires system UID");
    }
}
