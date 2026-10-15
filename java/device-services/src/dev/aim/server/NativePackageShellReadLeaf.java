package dev.aim.server;

import android.content.Context;
import android.content.Intent;
import android.content.pm.*;
import android.content.res.Resources;
import android.content.res.AssetManager;
import android.os.*;
import android.permission.PermissionManager;
import java.io.*;
import java.util.*;

/** Codec/resource leaves; package policy and resolution remain native. */
public final class NativePackageShellReadLeaf extends IPackageShellReadLeaf.Stub {
    private final Context context;
    private final PackageSnapshots.Store packages;
    public NativePackageShellReadLeaf(Context context, PackageSnapshots.Store packages) {
        this.context=Objects.requireNonNull(context);this.packages=Objects.requireNonNull(packages);
    }
    private static void enforce(){if(Binder.getCallingUid()!=1000)throw new SecurityException("Native shell owner required");}
    private static String[] lines(String text){return text.split("\\n",-1).length==1&&text.isEmpty()?new String[0]:text.substring(0,text.endsWith("\n")?text.length()-1:text.length()).split("\\n",-1);}
    @Override public int getMaxSupportedUsers(){enforce();return UserManager.getMaxSupportedUsers();}
    @Override public byte[] parseIntent(String[] arguments) {
        enforce();Objects.requireNonNull(arguments);
        final byte[][] result=new byte[1][];
        ShellCommand parser=new ShellCommand(){
            @Override public void onHelp(){}
            @Override public int onCommand(String ignored){
                final int[] user={-2}, flags={0};final boolean[] brief={false}, components={false};
                try {
                    Intent intent=Intent.parseCommandArgs(this,new Intent.CommandOptionHandler(){
                      @Override public boolean handleOption(String option,ShellCommand command){
                        switch(option){
                            case "--user":user[0]=UserHandle.parseUserArg(command.getNextArgRequired());return true;
                            case "--brief":brief[0]=true;return true;
                            case "--components":components[0]=true;return true;
                            case "--query-flags":flags[0]=Integer.decode(command.getNextArgRequired());return true;
                            default:return false;
                        }
                      }
                    });
                    Parcel out=Parcel.obtain();try{out.writeInt(user[0]);out.writeInt(flags[0]);out.writeBoolean(brief[0]);out.writeBoolean(components[0]);intent.writeToParcel(out,0);result[0]=out.marshall();}finally{out.recycle();}
                    return 0;
                }catch(java.net.URISyntaxException error){throw new IllegalArgumentException(error.getMessage(),error);}
            }
        };
        String[] all=new String[arguments.length+1];all[0]="parse-intent";System.arraycopy(arguments,0,all,1,arguments.length);
        parser.exec(this,null,null,null,all,null,null);
        return Objects.requireNonNull(result[0]);
    }
    @Override public String[] dumpResolveInfo(byte[] record,String prefix){
        enforce();Parcel in=Parcel.obtain();try{in.unmarshall(record,0,record.length);in.setDataPosition(0);ResolveInfo info=ResolveInfo.CREATOR.createFromParcel(in);if(in.dataAvail()!=0)throw new IllegalArgumentException("ResolveInfo tail");StringWriter text=new StringWriter();info.dump(new android.util.PrintWriterPrinter(new PrintWriter(text)),prefix);return lines(text.toString());}finally{in.recycle();}
    }
    @Override public String[] listInstrumentation(boolean showSourceDir, String targetPackage, int uid, int pid) throws RemoteException {
        enforce();
        try (var capture = packages.computer()) {
            var query = capture.readQueries(uid, pid);
            var instruments = new ArrayList<>(query.queryInstrumentationAsUser(
                    targetPackage, PackageManager.MATCH_KNOWN_PACKAGES, UserHandle.USER_SYSTEM).getList());
            instruments.sort((first, second) -> first.targetPackage.compareTo(second.targetPackage));
            var output = new ArrayList<String>(instruments.size());
            for (var info : instruments) {
                String component = new android.content.ComponentName(info.packageName, info.name).flattenToShortString();
                output.add("instrumentation:" + (showSourceDir ? info.sourceDir + "=" : "")
                        + component + " (target=" + info.targetPackage + ")");
            }
            return output.toArray(String[]::new);
        }
    }
    @Override public String[] listPermissions(boolean groups,boolean labels,boolean summary,int minimum,int maximum,String group,int uid,int pid)throws RemoteException{
        enforce();PermissionManager manager=Objects.requireNonNull(context.getSystemService(PermissionManager.class));
        var captured=packages.computer();
        IPackageManager pm=captured.readQueries(uid,pid);
        StringWriter text=new StringWriter();PrintWriter out=new PrintWriter(text);List<String> names=new ArrayList<>();
        if(groups){for(PermissionGroupInfo info:manager.getAllPermissionGroups(0))names.add(info.name);names.add(null);}else names.add(group);
        Map<String,Resources> resources=new HashMap<>();
        try{
            for(int i=0;i<names.size();i++){
                String name=names.get(i),prefix="";
                if(groups){
                    if(i>0)out.println();
                    if(name!=null){PermissionGroupInfo info=pm.getPermissionGroupInfo(name,0);
                        if(summary){Resources res=resources(pm,info,resources);out.print((res!=null?load(info,info.labelRes,info.nonLocalizedLabel,res):info.name)+": ");}
                        else{out.println((labels?"+ ":"")+"group:"+info.name);if(labels){out.println("  package:"+info.packageName);Resources res=resources(pm,info,resources);if(res!=null){out.println("  label:"+load(info,info.labelRes,info.nonLocalizedLabel,res));out.println("  description:"+load(info,info.descriptionRes,info.nonLocalizedDescription,res));}}}
                    }else out.println((labels&&!summary?"+ ":"")+"ungrouped:");
                    prefix="  ";
                }
                List<PermissionInfo> permissions=manager.queryPermissionsByGroup(name,0);boolean first=true;
                if(permissions!=null)for(PermissionInfo info:permissions){if(groups&&name==null&&info.group!=null)continue;int base=info.protectionLevel&PermissionInfo.PROTECTION_MASK_BASE;if(base<minimum||base>maximum)continue;
                    if(summary){if(!first)out.print(", ");first=false;Resources res=resources(pm,info,resources);out.print(res!=null?load(info,info.labelRes,info.nonLocalizedLabel,res):info.name);}
                    else{out.println(prefix+(labels?"+ ":"")+"permission:"+info.name);if(labels){out.println(prefix+"  package:"+info.packageName);Resources res=resources(pm,info,resources);if(res!=null){out.println(prefix+"  label:"+load(info,info.labelRes,info.nonLocalizedLabel,res));out.println(prefix+"  description:"+load(info,info.descriptionRes,info.nonLocalizedDescription,res));}out.println(prefix+"  protectionLevel:"+PermissionInfo.protectionToString(info.protectionLevel));}}
                }
                if(summary)out.println();
            }
            return lines(text.toString());
        }catch(RemoteException error){throw error.rethrowFromSystemServer();}finally{captured.close();}
    }
    private static String load(PackageItemInfo info,int id,CharSequence literal,Resources resources){if(literal!=null)return literal.toString();if(id!=0)try{return resources.getString(id);}catch(Resources.NotFoundException missing){}return null;}
    private static Resources resources(IPackageManager pm,PackageItemInfo info,Map<String,Resources> cache)throws RemoteException{
        Resources result=cache.get(info.packageName);if(result!=null)return result;
        ApplicationInfo app=pm.getApplicationInfo(info.packageName,PackageManager.MATCH_DISABLED_COMPONENTS|PackageManager.MATCH_HIDDEN_UNTIL_INSTALLED_COMPONENTS|PackageManager.MATCH_DISABLED_UNTIL_USED_COMPONENTS,0);
        if(app==null){android.util.Slog.e("PackageManager","Failed to get ApplicationInfo for package name("+info.packageName+").");return null;}
        AssetManager assets=new AssetManager();assets.addAssetPath(app.publicSourceDir);result=new Resources(assets,null,null);cache.put(info.packageName,result);return result;
    }
}
