package com.android.server.content;
/** Compile-only declaration of the explicit image seam and original query. */
public class ContentService extends android.content.IContentService.Stub {
    public void registerContentObserver(android.net.Uri uri, boolean descendants,
            android.database.IContentObserver observer, int user, int sdk) { throw new RuntimeException("stub"); }
    public ContentService(android.content.Context context, boolean factoryTest, SyncAccounts accounts) { throw new RuntimeException("stub"); }
    public android.content.SyncAdapterType[] getSyncAdapterTypesAsUser(int userId) { throw new RuntimeException("stub"); }
}
