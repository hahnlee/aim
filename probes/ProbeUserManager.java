package android.os;

/** Single-user policy exposed by the detached Android app process. */
public final class ProbeUserManager extends UserManager {
    public ProbeUserManager() {
        super();
    }

    @Override
    public boolean isUserUnlocked() {
        return true;
    }

    @Override
    public Bundle getApplicationRestrictions(String packageName) {
        return new Bundle();
    }
}
