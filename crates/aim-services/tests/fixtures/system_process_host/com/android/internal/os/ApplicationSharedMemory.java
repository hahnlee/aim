package com.android.internal.os;
import dev.aim.server.SystemProcessHostTest;
/** Models the pinned mutable NonceStore prerequisite, without loading Android JNI. */
public final class ApplicationSharedMemory {
    private static ApplicationSharedMemory instance;
    public static ApplicationSharedMemory create() {
        SystemProcessHostTest.events.add("memory");
        return new ApplicationSharedMemory();
    }
    public static void setInstance(ApplicationSharedMemory memory) {
        if (instance != null) throw new IllegalStateException("already initialized");
        instance = memory;
    }
    public static ApplicationSharedMemory getInstance() {
        if (instance == null) throw new IllegalStateException(
                "unable to assign nonce handle: cache_key.system_server.is_compat_change_enabled");
        return instance;
    }
}
