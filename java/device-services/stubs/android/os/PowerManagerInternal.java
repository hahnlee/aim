// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

import java.util.function.Consumer;

public abstract class PowerManagerInternal {
    public PowerManagerInternal() { throw new RuntimeException("stub"); }
    public abstract void registerLowPowerModeObserver(int serviceType, Consumer<PowerSaveState> listener);
    public abstract PowerSaveState getLowPowerState(int serviceType);
}
