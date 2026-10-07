// Compile-only pinned image API; checked by the device-services build node.
package android.app.admin;
public abstract class DevicePolicyManagerInternal {
    public abstract boolean isUserOrganizationManaged(int userId);
}
