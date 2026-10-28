package android.content;
public final class Intent {
    public final String action;
    public String extra;
    public int user, flags;
    public Intent(String action) { this.action = action; }
    public Intent putExtra(String name, int value) { extra = name; user = value; return this; }
    public Intent addFlags(int flags) { this.flags |= flags; return this; }
}
