package com.android.server.pm;
public final class NativeComputer implements Computer,AutoCloseable {
    public final java.util.List<android.content.pm.ResolveInfo> modern=new java.util.ArrayList<>(),legacy=new java.util.ArrayList<>();
    public final java.util.Set<String> denied=new java.util.HashSet<>(),disabled=new java.util.HashSet<>();
    public final java.util.List<String> checks=new java.util.ArrayList<>();public int closes;
    public java.util.List<android.content.pm.ResolveInfo> queryDomainVerificationReceivers(android.content.Intent intent,String type,long flags,int user){
        if(flags!=0x1c0000L||user!=0)throw new AssertionError("query scope");
        if(intent.action.equals("android.intent.action.DOMAINS_NEED_VERIFICATION")){if(type!=null)throw new AssertionError(type);return modern;}
        if(!intent.action.equals("android.intent.action.INTENT_FILTER_NEEDS_VERIFICATION")||!"application/vnd.android.package-archive".equals(type))throw new AssertionError(intent.action);
        return legacy;
    }
    public int checkDomainVerificationPermission(String permission,String name,int user){checks.add(permission+":"+name+":"+user);return denied.contains(name)?-1:0;}
    public boolean isComponentEffectivelyEnabled(android.content.pm.ComponentInfo component,android.os.UserHandle user){return !disabled.contains(component.packageName);}
    public int getPackageUid(String name,long flags,int user){return name.equals("agent")?10123:-1;}
    public com.android.server.pm.pkg.AndroidPackage getPackage(String name){throw new AssertionError("unexpected code query");}
    public boolean filterAppAccess(String name,int uid,int user,boolean include){throw new AssertionError("unexpected filter");}
    public void close(){closes++;}
}
