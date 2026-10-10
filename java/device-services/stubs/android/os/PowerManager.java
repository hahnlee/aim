// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public final class PowerManager {
    public static final int FULL_WAKE_LOCK = 0x0000001a;

    public PowerManager(android.content.Context context, IPowerManager service, IThermalService thermalService, Handler handler) { throw new RuntimeException("stub"); }
    public WakeLock newWakeLock(int levelAndFlags, String tag) { throw new RuntimeException("stub"); }
    public boolean isInteractive() { throw new RuntimeException("stub"); }

    public final class WakeLock {
        WakeLock(int flags, String tag, String packageName, int displayId) { throw new RuntimeException("stub"); }
        public void acquire() { throw new RuntimeException("stub"); }
        public void release() { throw new RuntimeException("stub"); }
        public boolean isHeld() { throw new RuntimeException("stub"); }
    }
 public static final int THERMAL_STATUS_SEVERE=3;public int getCurrentThermalStatus(){throw new RuntimeException("stub");}
}
