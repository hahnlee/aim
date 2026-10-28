package android.content;
public abstract class Context {
    public abstract android.content.pm.PackageManager getPackageManager();
    public <T> T getSystemService(Class<T> type) {
        return type.cast(new android.os.UserManager());
    }
}
