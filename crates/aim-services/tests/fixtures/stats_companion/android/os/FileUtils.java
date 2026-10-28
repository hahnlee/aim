// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
public class FileUtils {
    public static void closeQuietly(AutoCloseable value) {
        if(value!=null) try { value.close(); } catch(Exception e) { throw new AssertionError(e); }
    }
}
