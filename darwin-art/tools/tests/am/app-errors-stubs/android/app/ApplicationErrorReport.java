package android.app;

/** Test stub. */
public class ApplicationErrorReport {
    public static class CrashInfo {
        public String exceptionClassName;
        public String exceptionMessage;
        public String throwFileName;
        public String throwClassName;
        public String throwMethodName;
        public int throwLineNumber;
        public String stackTrace;
    }

    public static class ParcelableCrashInfo extends CrashInfo {}
}
