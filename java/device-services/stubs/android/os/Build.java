// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.os;

public class Build {
    public static final boolean IS_ENG;
    public static final boolean IS_USERDEBUG;
    static { IS_ENG = false; IS_USERDEBUG = false; }
    public static class VERSION {
        public static final int RESOURCES_SDK_INT;
        public static String INCREMENTAL;
        static { RESOURCES_SDK_INT = 0; }
    }
    public static boolean isDebuggable() { throw new RuntimeException("stub"); }
    public static final String[] SUPPORTED_64_BIT_ABIS = null;
    public static class VERSION_CODES {
        public static final int R = 30;
        public static final int CUR_DEVELOPMENT = 10000;
        public static final int TIRAMISU = 33;
    }
    public static final String[] SUPPORTED_ABIS=null;
    public static final String[] SUPPORTED_32_BIT_ABIS=null;
 public static boolean IS_DEBUGGABLE=false;
 public static final String TYPE=null;
}
