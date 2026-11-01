package com.android.server.content;

import android.content.Context;
import android.content.SyncAdapterType;
import android.os.Binder;
import android.os.Parcel;

/** Private construction capability. Lifecycle and public sync activation remain #1460. */
public final class PrivateContentService extends Binder {
    private final ContentService content;
    public PrivateContentService(Context context, SyncAccounts accounts) {
        content = new ContentService(context, false, accounts);
        // Force the original lazy manager after all local owners, without phase/unlock admission.
        content.getSyncAdapterTypesAsUser(0);
    }
    public SyncAdapterType[] getSyncAdapterTypes(int userId) {
        return content.getSyncAdapterTypesAsUser(userId);
    }
    @Override protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) {
        throw new IllegalStateException("private sync forwarding inactive (#1460)");
    }
}
