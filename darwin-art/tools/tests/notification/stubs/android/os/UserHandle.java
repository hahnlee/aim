package android.os;

/** Test stub. */
public final class UserHandle {
    public final int uid;
    private UserHandle(int uid) { this.uid = uid; }
    public static UserHandle getUserHandleForUid(int uid) { return new UserHandle(uid); }
}
