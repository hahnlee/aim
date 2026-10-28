package android.app.admin;
public abstract class PolicyValue<V> {
    protected PolicyValue(V value) {}
    public V getValue() { throw new RuntimeException("stub"); }
}
