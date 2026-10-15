// Compile-only pinned image API; never packaged as runtime implementation.
package android.os;
public class BatteryManager {
 public static final int BATTERY_PROPERTY_STATUS=6,BATTERY_PROPERTY_CAPACITY=4,BATTERY_STATUS_DISCHARGING=3;
 public int getIntProperty(int id){throw new RuntimeException("stub");}
}
