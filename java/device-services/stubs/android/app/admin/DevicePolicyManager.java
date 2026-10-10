// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app.admin;

import android.content.ComponentName;
import android.content.Context;

public class DevicePolicyManager {
    public DevicePolicyManager(Context context, IDevicePolicyManager service) { throw new RuntimeException("stub"); }
    public boolean getScreenCaptureDisabled(ComponentName admin) { throw new RuntimeException("stub"); }
    public String getDeviceOwner(){throw new RuntimeException("stub");}
    public boolean packageHasActiveAdmins(String name,int user){throw new RuntimeException("stub");}
}
