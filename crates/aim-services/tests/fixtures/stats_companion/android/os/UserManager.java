// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
public class UserManager {
    public java.util.List<UserHandle> users=java.util.List.of(new UserHandle(0), new UserHandle(10));
    public java.util.List<UserHandle> getUserHandles(boolean excludeDying) {
        if(!excludeDying) throw new AssertionError(); return users;
    }
}
