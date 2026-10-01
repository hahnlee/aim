// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.os.UserHandle;

public final class PendingIntent {
    public static final int FLAG_MUTABLE = 1 << 25;

    public PendingIntent(android.content.IIntentSender target) { throw new RuntimeException("stub"); }
    public static PendingIntent getActivityAsUser(Context context, int requestCode, Intent intent, int flags, Bundle options, UserHandle user) { throw new RuntimeException("stub"); }
}
