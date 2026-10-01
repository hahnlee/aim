// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package com.android.server.twilight;

import android.os.Handler;

public interface TwilightManager {
    void registerListener(TwilightListener listener, Handler handler);
    void unregisterListener(TwilightListener listener);
    TwilightState getLastTwilightState();
}
