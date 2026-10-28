// Host-only Android boundary double; production policy is compiled unchanged.
package android.os;
public class UserHandle {
    private final int id;
    public UserHandle(int id) { this.id=id; }
    public int getIdentifier() { return id; }
}
