package android.app.admin;
public abstract class DevicePolicyManagerInternal {
    public abstract int getDeviceOwnerUserId();
    public abstract android.content.ComponentName getDeviceOwnerComponent(boolean callingUser);
    public abstract android.content.ComponentName getProfileOwnerAsUser(int user);
}
