// Compile-only pinned image API; never packaged.
package android.app;
public final class ProcessMemoryState {
    public final int uid, pid, oomScore, mHostingComponentTypes, mHistoricalHostingComponentTypes;
    public final String processName;
    public final boolean hasForegroundServices;
    public ProcessMemoryState(int uid, int pid, String name, int oom, boolean foreground, int hosting, int historical) { throw new RuntimeException("stub"); }
}
