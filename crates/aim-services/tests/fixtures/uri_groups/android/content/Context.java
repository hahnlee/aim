package android.content;
public final class Context {
    public boolean granted = true;
    public int checks;
    public void enforcePermission(String permission, int pid, int uid, String message) {
        if (!permission.equals("android.permission.DOMAIN_VERIFICATION_AGENT")
                || pid != -1 || uid != 10100
                || !message.equals("Caller 10100 does not hold " + permission)) {
            throw new AssertionError("wrong permission/caller");
        }
        checks++;
        if (!granted) throw new SecurityException(message);
    }
}
