package android.content;

@interface NonNull {}
@interface Nullable {}

public class ComponentName {
    final String pkg, cls;
    public ComponentName(String pkg, String cls) { this.pkg = pkg; this.cls = cls; }
    // ORIGINAL_METHODS
}
