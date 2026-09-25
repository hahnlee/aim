package android.content;

import android.net.Uri;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Objects;
import java.util.Set;

public class Intent {
    public static final String ACTION_BATTERY_CHANGED = "android.intent.action.BATTERY_CHANGED";
    public static final String ACTION_BATTERY_LOW = "android.intent.action.BATTERY_LOW";
    public static final String ACTION_BATTERY_OKAY = "android.intent.action.BATTERY_OKAY";
    public static final String ACTION_POWER_CONNECTED =
            "android.intent.action.ACTION_POWER_CONNECTED";
    public static final String ACTION_POWER_DISCONNECTED =
            "android.intent.action.ACTION_POWER_DISCONNECTED";
    public static final int FLAG_RECEIVER_REGISTERED_ONLY = 0x40000000;
    public static final int FLAG_RECEIVER_REPLACE_PENDING = 0x20000000;

    private ComponentName component;
    private String action;
    private String packageName;
    private Set<String> categories;
    private int flags;
    private final Map<String, Object> extras = new HashMap<>();

    public Intent() {}
    public Intent(String action) { this.action = action; }
    public Intent(Intent other) {
        component = other.component;
        action = other.action;
        packageName = other.packageName;
        categories = other.categories == null ? null : new HashSet<>(other.categories);
        flags = other.flags;
        extras.putAll(other.extras);
    }
    public Intent setComponent(ComponentName value) { component = value; return this; }
    public ComponentName getComponent() { return component; }
    public String getAction() { return action; }
    public Intent setPackage(String value) { packageName = value; return this; }
    public String getPackage() { return packageName; }
    public Set<String> getCategories() { return categories; }
    public String getScheme() { return null; }
    public Uri getData() { return null; }
    public String getType() { return null; }
    public boolean hasFileDescriptors() { return false; }
    public Intent addFlags(int value) { flags |= value; return this; }
    public int getFlags() { return flags; }
    public Intent putExtra(String name, int value) { extras.put(name, value); return this; }
    public Intent putExtra(String name, boolean value) { extras.put(name, value); return this; }
    public Intent putExtra(String name, String value) { extras.put(name, value); return this; }
    public int getIntExtra(String name, int fallback) {
        Object value = extras.get(name);
        return value instanceof Integer ? (Integer) value : fallback;
    }
    public boolean getBooleanExtra(String name, boolean fallback) {
        Object value = extras.get(name);
        return value instanceof Boolean ? (Boolean) value : fallback;
    }
    public boolean filterEquals(Intent other) {
        return other != null && Objects.equals(action, other.action)
                && Objects.equals(packageName, other.packageName)
                && Objects.equals(component, other.component)
                && Objects.equals(categories, other.categories);
    }

    public static final class FilterComparison {
        private final Intent intent;
        public FilterComparison(Intent value) { intent = new Intent(value); }
        @Override public boolean equals(Object other) {
            if (!(other instanceof FilterComparison)) return false;
            return java.util.Objects.equals(intent.component,
                    ((FilterComparison) other).intent.component);
        }
        @Override public int hashCode() { return java.util.Objects.hashCode(intent.component); }
    }
}
