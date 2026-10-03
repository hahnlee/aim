// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm.permission;
public final class LegacyPermissionState {
    public LegacyPermissionState() { throw new RuntimeException("stub"); }
    public void copyFrom(LegacyPermissionState other) { throw new RuntimeException("stub"); }
    public void reset() { throw new RuntimeException("stub"); }
    public boolean isMissing(int userId) { throw new RuntimeException("stub"); }
    public void setMissing(boolean missing, int userId) { throw new RuntimeException("stub"); }
    public java.util.Collection<PermissionState> getPermissionStates(int userId) { throw new RuntimeException("stub"); }
    public PermissionState getPermissionState(String name, int userId) { throw new RuntimeException("stub"); }
    public void putPermissionState(PermissionState state, int userId) { throw new RuntimeException("stub"); }
    public static final class PermissionState {
        public PermissionState(String name, boolean runtime, boolean granted, int flags) { throw new RuntimeException("stub"); }
        public String getName() { throw new RuntimeException("stub"); }
        public boolean isRuntime() { throw new RuntimeException("stub"); }
        public boolean isGranted() { throw new RuntimeException("stub"); }
        public int getFlags() { throw new RuntimeException("stub"); }
    }
}
