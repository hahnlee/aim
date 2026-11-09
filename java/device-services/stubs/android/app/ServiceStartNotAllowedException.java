package android.app;
@SuppressWarnings("serial")
public abstract class ServiceStartNotAllowedException extends IllegalStateException {
    ServiceStartNotAllowedException(String message) { super(message); }
}
