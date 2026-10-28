package android.content.pm;
public final class ComponentInfo {
    public final String packageName;
    public ComponentInfo(String name){packageName=name;}
    public android.content.ComponentName getComponentName(){return new android.content.ComponentName(packageName,"Receiver");}
}
