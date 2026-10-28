// Host JVM test double; production links the retained image classes.
package android.content;
public class Context {
 public boolean updateAllowed;
 public int checks;
 public void enforceCallingOrSelfPermission(String permission,String message) {
  if (!permission.equals("android.permission.UPDATE_DEVICE_STATS")) throw new AssertionError(permission);
  checks++;
  if (!updateAllowed) throw new SecurityException("UPDATE_DEVICE_STATS");
 }
}
