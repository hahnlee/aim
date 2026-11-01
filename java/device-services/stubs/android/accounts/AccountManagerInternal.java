// Compile-only pinned local contract, checked against the image.
package android.accounts;
import android.os.RemoteCallback;
public abstract class AccountManagerInternal {
    public interface OnAppPermissionChangeListener {
        void onAppPermissionChanged(Account account, int uid);
    }
    public abstract boolean hasAccountAccess(Account account, int uid);
    public abstract void addOnAppPermissionChangeListener(OnAppPermissionChangeListener listener);
    public abstract void requestAccountAccess(Account account, String packageName, int userId, RemoteCallback callback);
    public abstract byte[] backupAccountAccessPermissions(int userId);
    public abstract void restoreAccountAccessPermissions(byte[] data, int userId);
}
