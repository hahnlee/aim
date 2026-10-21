// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.nearby;

import android.content.Context;
import android.nearby.INearbyManager;

public class NearbyService extends INearbyManager.Stub {
    public NearbyService(Context context) { throw new RuntimeException("stub"); }
    public void onBootPhase(int phase) { throw new RuntimeException("stub"); }
}
