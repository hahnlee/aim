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

    private String action;
    private final Set<String> categories = new java.util.LinkedHashSet<>();
    private ComponentName component;
    private int intentFlags;
    private static AssertionError unsupported() { return new AssertionError("unexercised Intent boundary"); }
    public Intent() {}
    public Intent(String action) { this.action=action; }
    public Intent setComponent(ComponentName value) { component = value; return this; }
    public Intent setDataAndType(Uri data, String type) {
        if (data != null || type != null) throw unsupported();
        return this;
    }
    public void writeToParcel(Parcel out, int flags) {
        out.writeString8(action);
        out.writeInt(0); // data URI
        out.writeString8(null); // type
        out.writeString8(null); // identifier
        out.writeInt(intentFlags); out.writeInt(0); // flags, extended flags
        out.writeString8(null); // package
        out.writeString(component == null ? null : component.pkg);
        if (component != null) out.writeString(component.cls);
        out.writeInt(0); out.writeInt(categories.size());
        for (String category : categories) out.writeString8(category);
        out.writeInt(0); out.writeInt(0); // selector, clip
        out.writeInt(-2); out.writeInt(-1); // content user, extras
        out.writeInt(0); out.writeInt(0); // original intent, creator token
    }
    public Intent setAction(String value) { action=value;return this; }
    public Intent setIdentifier(String value) { throw unsupported(); }
    public Intent addCategory(String value) { categories.add(value);return this; }
    public Intent putExtra(String key, Object value) { throw unsupported(); }
    public Intent setPackage(String value) { throw unsupported(); }
    public Intent setFlags(int value) { intentFlags=value;return this; }
    public Intent addFlags(int value) { intentFlags|=value;return this; }
    public void setSelector(Intent value) { throw unsupported(); }
    public static Intent parseUri(String value, int flags) throws URISyntaxException { throw unsupported(); }
    public Bundle getExtras() { throw unsupported(); }
    public Intent replaceExtras(Bundle value) { throw unsupported(); }
    public String getAction() { return action; }
    public Set<String> getCategories() { return categories.isEmpty() ? null : categories; }
    public void removeCategory(String value) { throw unsupported(); }
    public int fillIn(Intent value, int flags) { throw unsupported(); }
}
