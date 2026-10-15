// Compile-only pinned original class. Runtime uses the original framework implementation.
package com.android.server.pm.pkg.mutate;
public class PackageStateMutator {
    public PackageStateMutator(java.util.function.Function<String, com.android.server.pm.PackageSetting> active, java.util.function.Function<String, com.android.server.pm.PackageSetting> disabled) { throw new RuntimeException("stub"); }
    public Result generateResult(InitialState initial, int sequence) { throw new RuntimeException("stub"); }
    public void onFinished() { throw new RuntimeException("stub"); }
    public InitialState initialState(int sequence) { throw new RuntimeException("stub"); }
    public static class Result {
        public static final Result SUCCESS = null;
        public Result(boolean committed, boolean packages, boolean state, boolean specificNull) { throw new RuntimeException("stub"); }
    }
    public static class InitialState {
        public InitialState(int count, long sequence) { throw new RuntimeException("stub"); }
    }
}
