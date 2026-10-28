package android.app;
@SuppressWarnings("serial")
public abstract class ServiceStartNotAllowedException extends IllegalStateException {
    public ServiceStartNotAllowedException(String message) { super(message); }
}
