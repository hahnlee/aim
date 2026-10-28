package android.content.pm;
public final class UserProperties {
    private final int access;
    public UserProperties(int access) { this.access = access; }
    public int getCrossProfileIntentFilterAccessControl() { return access; }
}
