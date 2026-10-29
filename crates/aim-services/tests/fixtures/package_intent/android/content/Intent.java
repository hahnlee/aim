package android.content;

import android.net.Uri;
import android.os.Bundle;
import android.os.Parcel;
import android.os.ShellCommand;
import java.net.URISyntaxException;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.Set;

/** Value/parcel boundary for explicit-component shell tests. Syntax is injected
 * unchanged from hash-pinned AOSP; unexercised Android operations fail closed. */
public class Intent {
    public interface CommandOptionHandler {
        boolean handleOption(String option, ShellCommand command);
    }
    // ORIGINAL_CONSTANTS
    // ORIGINAL_METHODS

    private ComponentName component;
    private static AssertionError unsupported() { return new AssertionError("unexercised Intent boundary"); }
    public Intent() {}
    public Intent(String action) { throw unsupported(); }
    public Intent setComponent(ComponentName value) { component = value; return this; }
    public Intent setDataAndType(Uri data, String type) {
        if (data != null || type != null) throw unsupported();
        return this;
    }
    public void writeToParcel(Parcel out, int flags) {
        if (component == null) throw unsupported();
        out.writeString8(null); // action
        out.writeInt(0); // data URI
        out.writeString8(null); // type
        out.writeString8(null); // identifier
        out.writeInt(0); out.writeInt(0); // flags, extended flags
        out.writeString8(null); // package
        out.writeString(component.pkg); out.writeString(component.cls);
        for (int i = 0; i < 4; i++) out.writeInt(0); // bounds, categories, selector, clip
        out.writeInt(-2); out.writeInt(-1); // content user, extras
        out.writeInt(0); out.writeInt(0); // original intent, creator token
    }
    public Intent setAction(String value) { throw unsupported(); }
    public Intent setIdentifier(String value) { throw unsupported(); }
    public Intent addCategory(String value) { throw unsupported(); }
    public Intent putExtra(String key, Object value) { throw unsupported(); }
    public Intent setPackage(String value) { throw unsupported(); }
    public Intent setFlags(int value) { throw unsupported(); }
    public Intent addFlags(int value) { throw unsupported(); }
    public void setSelector(Intent value) { throw unsupported(); }
    public static Intent parseUri(String value, int flags) throws URISyntaxException { throw unsupported(); }
    public Bundle getExtras() { throw unsupported(); }
    public Intent replaceExtras(Bundle value) { throw unsupported(); }
    public String getAction() { throw unsupported(); }
    public Set<String> getCategories() { throw unsupported(); }
    public void removeCategory(String value) { throw unsupported(); }
    public int fillIn(Intent value, int flags) { throw unsupported(); }
}
