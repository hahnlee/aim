package android.content;
public class ContentProvider {
    public static boolean isAuthorityRedirectedForCloneProfile(String authority) {
        return "media".equals(authority);
    }
}
