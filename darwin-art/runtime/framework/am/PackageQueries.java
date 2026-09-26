package dev.darwinart.runtime.am;

import android.content.ComponentName;
import android.content.pm.ServiceInfo;

/**
 * The PackageManagerService queries the activity manager's services and job
 * scheduling make; {@link ApplicationPackages} answers them from PMS.
 */
public interface PackageQueries {
    /**
     * PackageManagerInternal.resolveService for an explicit component: the
     * declared service for {@code userId}, or null when it is not installed.
     */
    ServiceInfo service(ComponentName component, int userId);

    /**
     * PackageManagerInternal.addIsolatedUid, as ProcessList registers an
     * isolated process: PMS answers the isolated uid as its owner.
     */
    void addIsolatedUid(int isolatedUid, int ownerUid);

    /** PackageManagerInternal.removeIsolatedUid once the isolated uid is released. */
    void removeIsolatedUid(int isolatedUid);
}
