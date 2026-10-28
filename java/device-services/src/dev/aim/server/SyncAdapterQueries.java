package dev.aim.server;

import android.content.Context;
import android.content.SyncAdapterType;
import android.content.SyncAdaptersCache;
import android.os.Binder;
import android.os.Parcel;
import android.os.RemoteException;
import android.text.TextUtils;
import java.util.ArrayList;

/** SyncManager.getSyncAdapterTypes, android-16.0.0_r1. ContentService's native
 * endpoint checks cross-user access. Retain the original cache (metadata,
 * persisted UID ownership and package/user receivers), without running syncs.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
public final class SyncAdapterQueries extends ISyncAdapterQueries.Stub {
    private final Context context;
    private final ISystemProcess host;
    private SyncAdaptersCache adapters;

    public SyncAdapterQueries(Context context, ISystemProcess host) {
        this.context = context;
        this.host = host;
    }

    private synchronized SyncAdaptersCache adapters() {
        if (adapters == null) adapters = new SyncAdaptersCache(context);
        return adapters;
    }

    @Override public byte[] getSyncAdapterTypes(int callingUid, int userId) throws RemoteException {
        Bridge.enforceSystemUid();
        long token = Binder.clearCallingIdentity();
        Parcel out = Parcel.obtain();
        try {
            var serviceInfos = adapters().getAllServices(userId);
            var types = new ArrayList<SyncAdapterType>(serviceInfos.size());
            for (var serviceInfo : serviceInfos) {
                String packageName = serviceInfo.type.getPackageName();
                if (!TextUtils.isEmpty(packageName)
                        && host.filterSyncAdapterPackageAccess(packageName, callingUid, userId)) continue;
                types.add(serviceInfo.type);
            }
            out.writeTypedArray(types.toArray(new SyncAdapterType[] {}), 0);
            return out.marshall();
        } finally {
            out.recycle();
            Binder.restoreCallingIdentity(token);
        }
    }
}
