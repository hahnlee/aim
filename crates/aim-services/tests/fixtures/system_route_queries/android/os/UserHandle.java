package android.os;
public final class UserHandle {
 public final int id;
 private UserHandle(int id){this.id=id;}
 public static int getUserId(int uid){return uid/100000;}
 public static UserHandle of(int id){return new UserHandle(id);}
}
