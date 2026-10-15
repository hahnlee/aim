// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public class SystemProperties {
    public static Handle find(String name) { throw new RuntimeException("stub"); }
    public static class Handle {
 private Handle(long handle){throw new RuntimeException("stub");}
        public String get() { throw new RuntimeException("stub"); }
    }
    public static boolean getBoolean(String key, boolean def) { throw new RuntimeException("stub"); }
    private SystemProperties() { throw new RuntimeException("stub"); }
    public static void set(String key, String val) { throw new RuntimeException("stub"); }
 public static String get(String name){throw new RuntimeException("stub");}
}
