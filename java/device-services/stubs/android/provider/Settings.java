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
 public static final String COMPATIBILITY_MODE="compatibility_mode",FSTRIM_MANDATORY_INTERVAL="fstrim_mandatory_interval";
 public static long getLong(ContentResolver resolver,String name,long value){throw new RuntimeException("stub");}
        private Global() { throw new RuntimeException("stub"); }
        public static final String DEFAULT_INSTALL_LOCATION = "default_install_location";
        public static boolean putInt(ContentResolver cr, String name, int value) { throw new RuntimeException("stub"); }
        public static final String DEVICE_PROVISIONED = "device_provisioned";
        public static Uri getUriFor(String name) { throw new RuntimeException("stub"); }
        public static int getInt(ContentResolver cr, String name, int def) { throw new RuntimeException("stub"); }
    }
    public static final class Secure extends NameValueTable {
        private Secure() { throw new RuntimeException("stub"); }
        public static Uri getUriFor(String name) { throw new RuntimeException("stub"); }
        public static int getIntForUser(ContentResolver resolver, String name, int defaultValue, int user) { throw new RuntimeException("stub"); }
    }
}
