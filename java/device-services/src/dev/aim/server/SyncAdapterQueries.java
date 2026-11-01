package dev.aim.server;

import android.content.SyncAdapterType;
import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;
import android.text.TextUtils;
import java.util.ArrayList;

/** SyncManager.getSyncAdapterTypes, android-16.0.0_r1. ContentService's native
 * endpoint checks cross-user access. Retain the original cache (metadata,
 * persisted UID ownership and package/user receivers) from the private owner.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
public final class SyncAdapterQueries extends ISyncAdapterQueries.Stub {
    private final ISystemProcess host;
    private final java.util.function.IntFunction<SyncAdapterType[]> adapters;

    public SyncAdapterQueries(ISystemProcess host,
            java.util.function.IntFunction<SyncAdapterType[]> adapters) {
        this.host = host;
        this.adapters = java.util.Objects.requireNonNull(adapters);
    }

    @Override public byte[] getSyncAdapterTypes(int callingUid, int userId) throws RemoteException {
        Bridge.enforceSystemUid();
        long token = Binder.clearCallingIdentity();
        Parcel out = Parcel.obtain();
        try {
            var serviceInfos = adapters.apply(userId);
            var types = new ArrayList<SyncAdapterType>(serviceInfos.length);
            for (var serviceInfo : serviceInfos) {
                String packageName = serviceInfo.getPackageName();
                if (!TextUtils.isEmpty(packageName)
                        && host.filterSyncAdapterPackageAccess(packageName, callingUid, userId)) continue;
                types.add(serviceInfo);
            }
            out.writeTypedArray(types.toArray(new SyncAdapterType[] {}), 0);
            return out.marshall();
        } finally {
            out.recycle();
            Binder.restoreCallingIdentity(token);
        }
    }
}
