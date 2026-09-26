package dev.darwinart.runtime.system;

import android.os.Binder;
import android.os.Process;

/** System bootstrap may resolve local services before application admission. */
final class ServiceDirectoryAdmission {
    private final int systemPid = Process.myPid();
    private volatile boolean published;

    void enforceLookup() {
        if (!published && Binder.getCallingPid() != systemPid) {
            throw new SecurityException("System services are still initializing");
        }
    }

    void enforcePublication() {
        if (Binder.getCallingPid() != systemPid) {
            throw new SecurityException("Only the system process may add services");
        }
    }

    void publish() {
        if (Binder.getCallingPid() != systemPid) {
            throw new SecurityException("Only the system process may publish services");
        }
        if (published) throw new IllegalStateException("System services already published");
        published = true;
    }
}
