// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app.role;

import android.os.UserHandle;

public interface OnRoleHoldersChangedListener {
    void onRoleHoldersChanged(String roleName, UserHandle user);
}
