package android.content;
public class Context {
    public record Broadcast(Intent intent,android.os.UserHandle user,String permission,android.os.Bundle options) {}
    public final java.util.List<Broadcast> broadcasts=new java.util.ArrayList<>();
    public RuntimeException failure;
    public void sendBroadcastAsUser(Intent intent,android.os.UserHandle user,String permission,android.os.Bundle options){
        if(failure!=null)throw failure;broadcasts.add(new Broadcast(intent,user,permission,options));
    }
}
