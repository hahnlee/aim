// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content;

public abstract class ContentResolver {
    public ContentResolver(Context context) { throw new RuntimeException("stub"); }
    public static IContentService getContentService() { throw new RuntimeException("stub"); }
    public final void registerContentObserver(android.net.Uri uri, boolean notifyForDescendants, android.database.ContentObserver observer) { throw new RuntimeException("stub"); }
    public final void registerContentObserver(android.net.Uri uri, boolean notifyForDescendants, android.database.ContentObserver observer, int userHandle) { throw new RuntimeException("stub"); }
    public final void unregisterContentObserver(android.database.ContentObserver observer) { throw new RuntimeException("stub"); }
}
