package android.content;
public class Context {
    public record Broadcast(Intent intent, int user) {}
    public static final java.util.List<Broadcast> broadcasts = new java.util.ArrayList<>();
    public void sendBroadcastAsUser(Intent intent, android.os.UserHandle user) {
        broadcasts.add(new Broadcast(intent, user.getIdentifier()));
    }
}
