// Compile-only pinned original API.
package com.android.server.pm;
public class PersistentPreferredActivity extends WatchedIntentFilter {
    final boolean mIsSetByDpm;
    final android.content.ComponentName mComponent;
    PersistentPreferredActivity(android.content.IntentFilter filter,android.content.ComponentName activity,boolean byDpm) { throw new RuntimeException("stub"); }
    public PersistentPreferredActivity snapshot() { throw new RuntimeException("stub"); }
}
