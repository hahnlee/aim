package android.content;

public class Intent {
    private ComponentName component;
    public Intent() {}
    public Intent(Intent other) { component = other.component; }
    public Intent setComponent(ComponentName value) { component = value; return this; }
    public ComponentName getComponent() { return component; }

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
