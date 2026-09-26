package android.app;

/** Test stub: MemoryInfo with its hidden threshold fields. */
public class ActivityManager {
    public static class MemoryInfo implements android.os.Parcelable {
        public long advertisedMem;
        public long availMem;
        public long totalMem;
        public long threshold;
        public boolean lowMemory;
        long hiddenAppThreshold;
        long secondaryServerThreshold;
        long visibleAppThreshold;
        long foregroundAppThreshold;

        public long hidden(String name) {
            switch (name) {
                case "hiddenAppThreshold": return hiddenAppThreshold;
                case "secondaryServerThreshold": return secondaryServerThreshold;
                case "visibleAppThreshold": return visibleAppThreshold;
                default: return foregroundAppThreshold;
            }
        }
    }
}
