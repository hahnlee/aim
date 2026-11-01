package com.android.server.content;

import android.content.Context;
import android.content.SyncAdapterType;
import android.os.Binder;
import android.os.Parcel;

/** Private lifecycle capability; public sync forwarding remains inactive (#1460). */
public final class PrivateContentService extends Binder {
    private final ContentService content;
    private final SyncStorageOwner storage;
    public PrivateContentService(Context context, SyncAccounts accounts) {
        // Fail before the original store can validate or remove persisted authorities.
        java.util.Objects.requireNonNull(accounts.getAllAccountsForSystemProcess());
        storage = new SyncStorageOwner(android.os.Environment.getDataDirectory());
        content = new ContentService(context, false, accounts);
        // Force the original lazy manager after all local owners, without phase/unlock admission.
        content.getSyncAdapterTypesAsUser(0);
    }
    public void flush() { SyncStorageEngine.getSingleton().writeAllState(); }
    public void onBootPhase(int phase) { content.onBootPhase(phase); }
    public void onStartUser(int user) { content.onStartUser(user); }
    public void onUnlockUser(int user) { content.onUnlockUser(user); }
    public void replaySyncState(int user, Runnable complete) { content.replaySyncState(user, complete); }
    public SyncAdapterType[] getSyncAdapterTypes(int userId) {
        return content.getSyncAdapterTypesAsUser(userId);
    }
    @Override protected boolean onTransact(int code, Parcel data, Parcel reply, int flags) {
        throw new IllegalStateException("private sync forwarding inactive (#1460)");
    }
}
