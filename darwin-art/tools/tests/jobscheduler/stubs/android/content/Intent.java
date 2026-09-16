package android.content;

public class Intent {
    private ComponentName component;
    public Intent setComponent(ComponentName value) { component = value; return this; }
    public ComponentName getComponent() { return component; }
}
