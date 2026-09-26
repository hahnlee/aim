package dev.darwinart.system;

import android.os.Binder;
import dev.darwinart.runtime.system.SystemServiceFactory;

/** Process entry for the Darwin-hosted Android system service directory. */
public final class DarwinSystemServer {
    private DarwinSystemServer() {}

    public static Binder createServiceDirectory() {
        return SystemServiceFactory.create();
    }
}
