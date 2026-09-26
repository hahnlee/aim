package android.os;

/** Test stub. */
public final class UserHandle {
    public final int userId;
    private UserHandle(int userId) { this.userId = userId; }
    public static UserHandle of(int userId) { return new UserHandle(userId); }
}
