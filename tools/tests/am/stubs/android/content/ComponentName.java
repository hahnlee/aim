package android.content;

public final class ComponentName {
    private final String packageName;
    private final String className;

    public ComponentName(String packageName, String className) {
        this.packageName = packageName;
        this.className = className;
    }
    public String getPackageName() { return packageName; }
    public String getClassName() { return className; }
    public String flattenToString() { return packageName + "/" + className; }
    @Override public boolean equals(Object other) {
        if (!(other instanceof ComponentName)) return false;
        ComponentName value = (ComponentName) other;
        return packageName.equals(value.packageName) && className.equals(value.className);
    }
    @Override public int hashCode() { return 31 * packageName.hashCode() + className.hashCode(); }
}
