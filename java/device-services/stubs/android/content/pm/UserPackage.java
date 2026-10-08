// Compile-only pinned image API; checked by device-services.
package android.content.pm;
public final class UserPackage {
    public final int userId;
    public final String packageName;
    private UserPackage(int userId, String packageName) { throw new RuntimeException("stub"); }
    public static UserPackage of(int userId, String packageName) { throw new RuntimeException("stub"); }
    public static void removeFromCache(int userId,String packageName){throw new RuntimeException("stub");}
}
