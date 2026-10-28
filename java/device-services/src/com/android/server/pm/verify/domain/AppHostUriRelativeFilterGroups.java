/* Copyright (C) The Android Open Source Project, Apache License 2.0. */
package com.android.server.pm.verify.domain;

import android.content.Context;
import android.content.UriRelativeFilterGroup;
import android.content.UriRelativeFilterGroupParcel;
import android.content.pm.PackageManager.NameNotFoundException;
import android.os.Bundle;
import android.os.RemoteException;
import dev.aim.server.IAppHostDomainVerification;
import java.util.ArrayList;
import java.util.List;

/**
 * DomainVerificationService's android-16.0.0_r1 URI methods, adapted only at
 * the attached-package state boundary; all Bundle policy uses original classes.
 */
public final class AppHostUriRelativeFilterGroups {
    private AppHostUriRelativeFilterGroups() {}

    public static void set(Context context, int uid, IAppHostDomainVerification backend,
            String packageName, Bundle bundle) throws NameNotFoundException, RemoteException {
        // The system-UID bridge carries the authenticated caller explicitly.
        context.enforcePermission(android.Manifest.permission.DOMAIN_VERIFICATION_AGENT, -1, uid,
                "Caller " + uid + " does not hold "
                        + android.Manifest.permission.DOMAIN_VERIFICATION_AGENT);
        if (bundle.isEmpty()) {
            return;
        }
        String id = backend.getDomainVerificationPackageId(packageName);
        if (id == null) {
            throw DomainVerificationUtils.throwPackageUnavailable(packageName);
        }
        for (String domain : bundle.keySet()) {
            if (!DomainVerificationUtils.isValidDomain(domain)) {
                continue;
            }
            ArrayList<UriRelativeFilterGroupParcel> parcels =
                    bundle.getParcelableArrayList(domain, UriRelativeFilterGroupParcel.class);
            List<UriRelativeFilterGroup> groups = UriRelativeFilterGroup.parcelsToGroups(parcels);
            // Convert with the original constructors before publishing each key: an
            // exception on a later key retains earlier changes, just as in DVS.
            backend.setUriRelativeFilterGroup(packageName, id, domain,
                    UriRelativeFilterGroup.groupsToParcels(groups));
        }
        // Original DVS does not scheduleWriteSettings for this setter.
    }

    public static Bundle get(IAppHostDomainVerification backend, int uid, String packageName,
            List<String> domains) throws RemoteException {
        if (backend.getDomainVerificationPackageId(packageName) == null) {
            return new Bundle();
        }
        // Original DVS dereferences the list only for an attached package.
        domains.size();
        return backend.getUriRelativeFilterGroups(uid, packageName, domains);
    }
}
