// Compile-only pinned image API, checked by the device-services build.
package android.app;
public abstract class PropertyInvalidatedCache<Q, R> {
    public PropertyInvalidatedCache(int maxEntries, String propertyName) { throw new RuntimeException("stub"); }
    public R recompute(Q query) { throw new RuntimeException("stub"); }
    public R query(Q query) { throw new RuntimeException("stub"); }
    public void invalidateCache() { throw new RuntimeException("stub"); }
}
