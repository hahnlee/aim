// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.os.UserHandle;

public class NotificationManager {
    public NotificationManager(android.content.Context context) { throw new RuntimeException("stub"); }
    public void notifyAsUser(String tag, int id, Notification notification, UserHandle user) { throw new RuntimeException("stub"); }
    public void cancelAsUser(String tag, int id, UserHandle user) { throw new RuntimeException("stub"); }
}
