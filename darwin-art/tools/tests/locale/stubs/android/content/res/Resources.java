package android.content.res;

/** Test-only Resources system singleton for the Binder wire test. */
public final class Resources {
    private static final Resources SYSTEM = new Resources();
    private final Configuration configuration = new Configuration();

    private Resources() {}

    public static Resources getSystem() {
        return SYSTEM;
    }

    public Configuration getConfiguration() {
        return configuration;
    }
}
