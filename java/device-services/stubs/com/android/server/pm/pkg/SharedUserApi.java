// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm.pkg;

import android.content.pm.SigningDetails;
import android.util.ArraySet;

public interface SharedUserApi {
    String getName();
    int getAppId();
    ArraySet<? extends PackageState> getPackageStates();
    boolean isPrivileged();
    int getSeInfoTargetSdkVersion();
    SigningDetails getSigningDetails();
}
