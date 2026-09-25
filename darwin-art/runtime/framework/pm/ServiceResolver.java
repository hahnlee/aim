package dev.darwinart.runtime.pm;

import android.content.ComponentName;
import android.content.pm.ServiceInfo;

/**
 * PackageManagerService.getServiceInfo for services the system itself binds
 * or schedules (ActiveServices, JobSchedulerService).
 */
public interface ServiceResolver {
    /** The declared service, or null when the package or service is not installed. */
    ServiceInfo service(ComponentName component);
}
