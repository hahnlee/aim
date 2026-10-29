package android.content.pm;
public final class ResolveInfo {
    public int priority; public final ComponentInfo component;
    public ResolveInfo(String name,int priority){this.priority=priority;component=new ComponentInfo(name);}
    public ComponentInfo getComponentInfo(){return component;}
}
