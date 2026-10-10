// Compile-only pinned image API; checked against original classes.
package android.compat;
public final class Compatibility {
    public interface BehaviorChangeDelegate { default boolean isChangeEnabled(long id) { throw new RuntimeException("stub"); } }
    public static void setBehaviorChangeDelegate(BehaviorChangeDelegate delegate) { throw new RuntimeException("stub"); }
    public static void clearBehaviorChangeDelegate() { throw new RuntimeException("stub"); }
    public static boolean isChangeEnabled(long id) { throw new RuntimeException("stub"); }
}
