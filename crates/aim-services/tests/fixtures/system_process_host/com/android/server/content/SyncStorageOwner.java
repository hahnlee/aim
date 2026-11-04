package com.android.server.content;
/** JVM startup scenarios simulate separate system process generations. */
public class SyncStorageOwner {
    public static int claims;
    public SyncStorageOwner(java.io.File data) { claims++; }
}
