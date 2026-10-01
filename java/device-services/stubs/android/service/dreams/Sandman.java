// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.service.dreams;

import android.content.Context;
import android.content.Intent;

public final class Sandman {
    private Sandman() { throw new RuntimeException("stub"); }
    public static boolean shouldStartDockApp(Context context, Intent intent) { throw new RuntimeException("stub"); }
    public static void startDreamWhenDockedIfAppropriate(Context context) { throw new RuntimeException("stub"); }
}
