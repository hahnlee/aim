package android.os;
public final class UserHandle {
    private final int id;
    private UserHandle(int id) { this.id = id; }
    public static UserHandle of(int id) { return new UserHandle(id); }
    public int getIdentifier() { return id; }
}
