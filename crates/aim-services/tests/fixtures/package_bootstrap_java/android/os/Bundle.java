package android.os;
public final class Bundle {
    private final java.util.Map<String,Object> values = new java.util.HashMap<>();
    public void putLong(String key, long value) { values.put(key, value); }
    public void putInt(String key, int value) { values.put(key, value); }
    public void putString(String key, String value) { values.put(key, value); }
    public long getLong(String key) { return (long) values.get(key); }
    public int getInt(String key) { return (int) values.get(key); }
    public String getString(String key) { return (String) values.get(key); }
}
