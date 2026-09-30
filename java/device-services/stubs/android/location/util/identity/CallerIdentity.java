// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.location.util.identity;

public final class CallerIdentity {
    private CallerIdentity(int uid, int pid, String packageName, String attributionTag,
            String listenerId) { throw new RuntimeException("stub"); }
    public int getUid() { throw new RuntimeException("stub"); }
    public int getPid() { throw new RuntimeException("stub"); }
    public String getPackageName() { throw new RuntimeException("stub"); }
    public String getAttributionTag() { throw new RuntimeException("stub"); }
}
