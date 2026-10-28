// Compile-only pinned API.
package android.app;
@SuppressWarnings("serial")
public final class MissingForegroundServiceTypeException extends ForegroundServiceTypeException {
    public MissingForegroundServiceTypeException(String message) { super(message); }
}
