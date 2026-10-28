package android.content;
public class PermissionChecker {
 public static final int PERMISSION_GRANTED=0;
 public static int result,calls;
 public static int checkPermissionForDataDelivery(Context c,String p,int pid,int uid,String name,String tag,String msg){
  if(pid!=71||uid!=210100||!"caller".equals(name)||tag!=null||android.os.Binder.cleared
    ||!p.equals("android.permission.MEDIA_ROUTING_CONTROL")
    ||!msg.equals("Checking permissions for registering manager in MediaRouter2ServiceImpl."))throw new AssertionError("delivery identity");
  calls++;return result;
 }
}
