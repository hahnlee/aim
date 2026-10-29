package android.os;
public record UserHandle(int id) {
    public static final int USER_SYSTEM=0;public static final UserHandle SYSTEM=new UserHandle(0);
    public static UserHandle of(int id){return new UserHandle(id);}
    public static int getUserId(int uid){return uid/100000;}
}
