package android.app;
@SuppressWarnings("serial")
public abstract class ForegroundServiceTypeException extends ServiceStartNotAllowedException {
    public ForegroundServiceTypeException(String message) { super(message); }
}
