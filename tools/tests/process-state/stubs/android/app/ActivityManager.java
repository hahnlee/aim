package android.app;

/** Test stub: MemoryInfo with its hidden (public @hide) threshold fields. */
public class ActivityManager {
    public static class MemoryInfo implements android.os.Parcelable {
        public long advertisedMem;
        public long availMem;
        public long totalMem;
        public long threshold;
        public boolean lowMemory;
        public long hiddenAppThreshold;
        public long secondaryServerThreshold;
        public long visibleAppThreshold;
        public long foregroundAppThreshold;

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
