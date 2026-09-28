package android.content;

/** Test-only permission boundary used by the focused host-Java test. */
public abstract class Context {
    public abstract void enforceCallingOrSelfPermission(String permission, String message);
}
