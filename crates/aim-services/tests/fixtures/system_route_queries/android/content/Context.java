package android.content;
import java.util.*;
public class Context {
 public final Set<String> granted=new HashSet<>();
 public final List<String> checks=new ArrayList<>();
 public int checkPermission(String p,int pid,int uid){
  if(pid!=71||uid!=210100||android.os.Binder.cleared)throw new AssertionError("lost caller");
  checks.add(p);return granted.contains(p)?0:-1;
 }
}
