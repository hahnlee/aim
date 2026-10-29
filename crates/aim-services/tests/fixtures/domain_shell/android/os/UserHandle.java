package android.os; public class UserHandle {public static final int USER_ALL=-1;public static int parseUserArg(String s){return s.equals("all")?-1:s.equals("current")?-2:Integer.parseInt(s);}}
