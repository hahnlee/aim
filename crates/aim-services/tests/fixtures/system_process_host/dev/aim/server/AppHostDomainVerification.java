package dev.aim.server;
/** Constructor double for PlatformCompat -> CompatConfig -> IpcDataCache invalidation. */
final class AppHostDomainVerification extends android.os.Binder {
    AppHostDomainVerification(android.content.Context context, ISystemProcess host) {
        com.android.internal.os.ApplicationSharedMemory.getInstance();
        SystemProcessHostTest.events.add("domain");
    }
}
