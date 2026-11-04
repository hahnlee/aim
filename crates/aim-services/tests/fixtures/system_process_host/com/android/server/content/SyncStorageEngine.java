package com.android.server.content;
public class SyncStorageEngine {
    public static int writes;
    private static final SyncStorageEngine owner = new SyncStorageEngine();
    public static SyncStorageEngine getSingleton() { return owner; }
    public void writeAllState() { writes++; }
}
