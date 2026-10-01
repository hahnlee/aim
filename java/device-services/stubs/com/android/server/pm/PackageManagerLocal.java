// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.pm;

import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.SharedUserApi;

import java.util.Map;

public interface PackageManagerLocal {
    UnfilteredSnapshot withUnfilteredSnapshot();

    interface UnfilteredSnapshot extends AutoCloseable {
        Map<String, PackageState> getPackageStates();
        Map<String, PackageState> getDisabledSystemPackageStates();
        Map<String, SharedUserApi> getSharedUsers();
        @Override
        void close();
    }
}
