import android.os.*;
import com.android.server.pm.verify.domain.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

/** Exact text oracle: production AppHostDomainShell + pinned parser/Debug/indent writer. */
public class DomainShellTest {
    static final String ID = "00000000-0000-0000-0000-000000000001";
    static final class Record {
        final ByteArrayOutputStream out = new ByteArrayOutputStream();
        void i(int n) { for (int j=0;j<4;j++) out.write(n >>> (8*j)); }
        void s(String s) { if(s==null){i(-1);return;} i(s.length());out.writeBytes(s.getBytes(StandardCharsets.UTF_16LE));out.write(0);out.write(0);pad(); }
        void pad(){while(out.size()%4!=0)out.write(0);}
        void strings(String... s){i(s.length);for(String v:s)s(v);}
    }
    static byte[] capture(int kind, String name, List<String> domains, int user, boolean hasUser) {
        if (!Objects.equals(name,"p") && name!=null) throw new AssertionError(name);
        if (hasUser && user!=0 && user!=-1) throw new AssertionError(user);
        Record p=new Record();p.i(1);
        if(kind==0){
            p.s("p");p.s(ID);p.i(0); // PackageCacher boundary returns a package with no signatures.
            p.strings("a.test","b.test");p.strings();p.strings("a.test","b.test","c.test");
            p.i(1);p.s("a.test");p.i(1);p.i(1);p.i(0);p.i(1);p.strings("c.test");
        } else {
            p.s(kind==1?"p":null);
            var requested=kind==1?List.of("a.test","b.test","c.test"):domains;
            p.i(requested.size());
            for(String d:requested){p.s(d);p.i(1);p.i(0);p.i(1);
                p.i(d.equals("a.test")?4:d.equals("b.test")?-1:3);p.strings("p");}
        }
        return p.out.toByteArray();
    }
    static final class Backend implements dev.aim.server.IAppHostDomainVerification {
        final List<String> writes = new ArrayList<>();
        RuntimeException failure;
        public void setShellStatus(String name,int state,List<String> domains){
            if(failure!=null)throw failure;
            if("missing".equals(name))throw new ServiceSpecificException(1);
            writes.add("status:"+name+":"+state+":"+domains);
        }
        public void resetShellState(List<String> names,int user,boolean userOnly){
            if(failure!=null)throw failure;
            writes.add("reset:"+names+":"+user+":"+userOnly);
        }
        public byte[] readShellState(int kind,String name,List<String> domains,int user,boolean hasUser){return capture(kind,name,domains,user,hasUser);}
        public void setShellUserSelection(String name,int user,boolean enabled,List<String> domains){
            if(name.equals("missing"))throw new ServiceSpecificException(1);
            writes.add("select:"+name+":"+user+":"+enabled+":"+domains);
        }
        public void setShellLinkHandling(String name,int user,boolean allowed){
            if("missing".equals(name))throw new ServiceSpecificException(1);
            writes.add("allowed:"+name+":"+user+":"+allowed);
        }
    }
    static final Backend backend = new Backend();
    static int run(boolean pm,String[] args,FileDescriptor out,FileDescriptor err){
        var shell=new AppHostDomainShell(backend,new DomainVerificationEnforcer(null),null);
        return shell.run(null,pm,args,new ParcelFileDescriptor(FileDescriptor.in),new ParcelFileDescriptor(out),new ParcelFileDescriptor(err));
    }
    static void check(boolean pm,String expected,String... args)throws Exception {checkResult(pm,0,expected,"",args);}
    static void checkResult(boolean pm,int statusExpected,String expected,String errorExpected,String... args)throws Exception {
        File o=File.createTempFile("domain-out", ".txt"),e=File.createTempFile("domain-err", ".txt");
        try(var out=new FileOutputStream(o);var err=new FileOutputStream(e)){
            int status=run(pm,args,out.getFD(),err.getFD());
            String actual=java.nio.file.Files.readString(o.toPath()),error=java.nio.file.Files.readString(e.toPath());
            if(status!=statusExpected||!actual.equals(expected)||!error.equals(errorExpected))throw new AssertionError(Arrays.toString(args)+" pm="+pm+" status="+status+"\n"+actual+"stderr="+error+"\nexpected:\n"+expected);
        }finally{o.delete();e.delete();}
    }
    static String links(boolean user){return "  p:\n    ID: "+ID+"\n    Signatures: null\n    Domain verification state:\n      a.test: verified\n      b.test: none\n"+(user?"    User 0:\n      Verification link handling allowed: true\n      Selection state:\n        Enabled:\n          c.test\n        Disabled:\n          a.test\n          b.test\n":"");}
    static String owners(String indent){return indent+"a.test:\n"+indent+"  User 0:\n"+indent+"    VERIFIED[4]:\n"+indent+"      p\n"+indent+"b.test:\n"+indent+"  User 0:\n"+indent+"    UNVERIFIED[-1]:\n"+indent+"      p\n"+indent+"c.test:\n"+indent+"  User 0:\n"+indent+"    USER_SELECTION[3]:\n"+indent+"      p\n";}
    public static void main(String[] args)throws Exception {
        if(args.length!=0){System.exit(run(false,args,FileDescriptor.out,FileDescriptor.err));}
        for(boolean pm:new boolean[]{false,true}){
            backend.writes.clear();
            check(pm,"","set-app-links-user-selection","--user","0","--package","p","true","c.test");
            check(pm,"","set-app-links-user-selection","--user","all","--package","p","false","all");
            check(pm,"","set-app-links-allowed","--user","0","--package","p","false");
            check(pm,"","set-app-links-allowed","--user","all","--package","all","true");
            var expectedWrites=List.of("select:p:0:true:[c.test]","select:p:-1:false:null","allowed:p:0:false","allowed:null:-1:true");
            if(!backend.writes.equals(expectedWrites))throw new AssertionError(backend.writes);
            checkResult(pm,1,"","Package not found: missing\n","set-app-links-user-selection","--user","0","--package","missing","true","all");
            checkResult(pm,1,"","Package not found: missing\n","set-app-links-allowed","--user","0","--package","missing","true");
            Binder.uid=10100;
            var shell=new AppHostDomainShell(backend,new DomainVerificationEnforcer(null),null);
            try {shell.setDomainVerificationUserSelectionInternal(0,"p",true,null);throw new AssertionError("permission bypass");}
            catch(SecurityException expected){if(!expected.getMessage().equals("Caller 10100 is not allowed to change internal state"))throw expected;}
            try {shell.setDomainVerificationLinkHandlingAllowedInternal("p",false,0);throw new AssertionError("permission bypass");}
            catch(SecurityException expected){if(!expected.getMessage().equals("Caller 10100 is not allowed to change internal state"))throw expected;}
            Binder.uid=2000;
            if(!backend.writes.equals(expectedWrites))throw new AssertionError("denied write reached backend");
            backend.writes.clear();
            for(String state:new String[]{"0","1","2","3","STATE_NO_RESPONSE","STATE_SUCCESS","STATE_APPROVED","STATE_DENIED"})
                check(pm,"","set-app-links","--package","p",state,"a.test","undeclared.test");
            check(pm,"","set-app-links","--package","ALL","1","all");
            check(pm,"","reset-app-links","p","q");
            check(pm,"","reset-app-links","ALL");
            check(pm,"","reset-app-links","--user","0","p");
            check(pm,"","reset-app-links","--user","all","all");
            var expectedStatus=new ArrayList<String>();
            for(int i=0;i<8;i++)expectedStatus.add("status:p:"+(i%4)+":[a.test, undeclared.test]");
            expectedStatus.addAll(List.of("status:null:1:null","reset:[p, q]:0:false",
                "reset:null:0:false","reset:[p]:0:true","reset:null:-1:true"));
            if(!backend.writes.equals(expectedStatus))throw new AssertionError(backend.writes);
            checkResult(pm,1,"","Package not found: missing\n","set-app-links","--package","missing","0","all");
            checkResult(pm,1,"","Error: no package specified\n","set-app-links","0","all");
            checkResult(pm,1,"","Invalid state option: 4\n","set-app-links","--package","p","4","all");
            checkResult(pm,1,"","No domains specified\n","set-app-links","--package","p","1");
            checkResult(pm,1,"","Error: unknown option: --invalid\n","reset-app-links","--invalid");
            checkResult(pm,1,"","Error: no package specified\n","reset-app-links");
            Binder.uid=10100;
            for(int operation=0;operation<3;operation++){
                try {
                    if(operation==0)shell.setDomainVerificationStatusInternal("p",1,null);
                    else if(operation==1)shell.clearDomainVerificationState(null);
                    else shell.clearUserStates(null,-1);
                    throw new AssertionError("permission bypass");
                }catch(SecurityException expected){if(!expected.getMessage().equals("Caller 10100 is not allowed to change internal state"))throw expected;}
            }
            Binder.uid=2000;
            if(!backend.writes.equals(expectedStatus))throw new AssertionError("denied write reached backend");
            backend.failure=new ServiceSpecificException(7);
            try {shell.setDomainVerificationStatusInternal("p",1,null);throw new AssertionError("swallowed status error");}
            catch(ServiceSpecificException expected){if(expected.errorCode!=7)throw expected;}
            try {shell.clearDomainVerificationState(null);throw new AssertionError("swallowed reset error");}
            catch(ServiceSpecificException expected){if(expected.errorCode!=7)throw expected;}
            backend.failure=null;
            check(pm,links(false),"get-app-links","p");
            check(pm,links(true),"get-app-links","--user","0","p");
            check(pm,links(true),"get-app-links","--user","all");
            check(pm,"  p:\n"+owners("    "),"get-app-link-owners","--user","0","--package","p");
            check(pm,owners("  "),"get-app-link-owners","--user","all","a.test","b.test","c.test");
        }
    }
}
