// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server;

import android.content.Context;

public abstract class SystemService {
    public static final int PHASE_SYSTEM_SERVICES_READY = 500;

    public SystemService(Context context) { throw new RuntimeException("stub"); }
    public abstract void onStart();
    public void onBootPhase(int phase) { throw new RuntimeException("stub"); }
}
