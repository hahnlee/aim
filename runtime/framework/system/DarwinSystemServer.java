package dev.aim.system;

import android.os.Binder;
import dev.aim.runtime.system.SystemServiceFactory;

/** Process entry for the Darwin-hosted Android system service directory. */
public final class DarwinSystemServer {
    private DarwinSystemServer() {}

    public static Binder createServiceDirectory() {
        return SystemServiceFactory.create();
    }
}
