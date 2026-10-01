// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.provider;

import android.content.ContentResolver;
import android.net.Uri;

public final class Settings {
    private Settings() { throw new RuntimeException("stub"); }

    public static class NameValueTable {
        public NameValueTable() { throw new RuntimeException("stub"); }
    }

    public static final class Global extends NameValueTable {
        private Global() { throw new RuntimeException("stub"); }
        public static final String DEVICE_PROVISIONED = "device_provisioned";
        public static Uri getUriFor(String name) { throw new RuntimeException("stub"); }
        public static int getInt(ContentResolver cr, String name, int def) { throw new RuntimeException("stub"); }
    }
}
