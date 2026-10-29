package dev.aim.server;
import android.content.pm.UserInfo;
/** UserManagerService's cold switcher policy over the native user inventory. */
interface IUserSwitcherPolicy {
    boolean isEnabled(boolean showEvenIfNotActionable, boolean switchRestricted,
            boolean addRestricted, boolean canReadRestrictions, boolean canQueryUsers,
            in UserInfo[] users);
}
