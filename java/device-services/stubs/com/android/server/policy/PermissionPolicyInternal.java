// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.policy;

import android.content.Intent;

public abstract class PermissionPolicyInternal {
    public abstract boolean isIntentToPermissionDialog(Intent intent);
}
