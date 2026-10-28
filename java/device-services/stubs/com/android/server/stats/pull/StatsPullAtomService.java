// Compile-only API of the original services.jar class; never packaged.
package com.android.server.stats.pull;

public final class StatsPullAtomService extends com.android.server.SystemService {
    public StatsPullAtomService(android.content.Context context) { super(context); }
    @Override public void onStart() { throw new RuntimeException("stub"); }
    @Override public void onBootPhase(int phase) { throw new RuntimeException("stub"); }
}
