package com.android.server.pm;

import android.os.Binder;
import android.os.Handler;
import android.os.RemoteException;
import android.util.Xml;
import com.android.server.pm.verify.domain.DomainVerificationService;
import dev.aim.server.IPackageDomainSettings;
import dev.aim.server.IPackageDomainSettingsChanged;
import java.io.*;
import java.util.Objects;
import java.util.function.Supplier;

/** Original DVS typed persistence codec and admitted-package lifecycle; no object copying. */
public final class NativeDomainSettings extends IPackageDomainSettings.Stub {
    private final DomainVerificationService domains;
    private final Supplier<NativeComputer> computers;
    private final Handler handler;
    private IPackageDomainSettingsChanged changes;
    private boolean seeded;
    private NativeComputer retained;
    private boolean closed;
    public NativeDomainSettings(DomainVerificationService domains,Supplier<NativeComputer> computers,Handler handler){
        this.domains=Objects.requireNonNull(domains);this.computers=Objects.requireNonNull(computers);this.handler=Objects.requireNonNull(handler);
    }
    private static void enforce(){if(Binder.getCallingUid()!=1000)throw new SecurityException("Native domain settings owner required");}
    @Override public synchronized void seed(byte[] nativeSettings,IPackageDomainSettingsChanged changes){
        enforce();if(seeded||closed)throw new IllegalStateException("original domain settings already seeded or closed");
        this.changes=Objects.requireNonNull(changes);
        var snapshot=computers.get();
        try{
            var parser=Xml.resolvePullParser(new ByteArrayInputStream(Objects.requireNonNull(nativeSettings)));
            int type;while((type=parser.next())!=org.xmlpull.v1.XmlPullParser.END_DOCUMENT){
                if(type!=org.xmlpull.v1.XmlPullParser.START_TAG)continue;
                if("domain-verifications".equals(parser.getName()))domains.readSettings(snapshot,parser);
                else if("domain-verifications-legacy".equals(parser.getName()))domains.readLegacySettings(parser);
            }
            // These are the actual admitted native PackageState objects/UUIDs,
            // exactly the same lifecycle addPackage original PMS invokes.
            for(var state:snapshot.getPackageStates().values())if(state.getPkg()!=null&&state.getDomainSetId()!=null)domains.addPackage(state,null);
            retained=snapshot;seeded=true;
        }catch(IOException|org.xmlpull.v1.XmlPullParserException|RuntimeException failure){snapshot.close();throw new IllegalStateException("original domain settings import failed",failure);}
    }
    @Override public synchronized byte[] capture(){
        enforce();if(!seeded||closed)throw new IllegalStateException("original domain settings not seeded or closed");
        return captureState();
    }
    private byte[] captureState(){
        try(var snapshot=computers.get()){
            var bytes=new ByteArrayOutputStream();var serializer=Xml.resolveSerializer(bytes);
            serializer.startDocument(null,true);serializer.startTag(null,"packages");
            domains.writeSettings(snapshot,serializer,false,-1);
            serializer.endTag(null,"packages");serializer.endDocument();return bytes.toByteArray();
        }catch(IOException failure){throw new IllegalStateException("original domain settings export failed",failure);}
    }
    @Override public synchronized void reconcilePackages(){
        enforce();if(!seeded||closed)throw new IllegalStateException("original domain settings not seeded or closed");
        var next=computers.get();boolean applied=false;
        try{
            var old=retained.getPackageStates();var current=next.getPackageStates();
            for(String name:old.keySet())if(!current.containsKey(name))domains.clearPackage(name);
            for(var entry:current.entrySet()){
                var setting=entry.getValue();if(setting.getPkg()==null||setting.getDomainSetId()==null)continue;
                var previous=old.get(entry.getKey());
                if(previous==null)domains.addPackage(setting,null);
                else if(!Objects.equals(previous.getDomainSetId(),setting.getDomainSetId())
                    || previous.getPkg()==null
                    || previous.getPkg().getLongVersionCode()!=setting.getPkg().getLongVersionCode()
                    || !Objects.equals(previous.getPkg().getBaseApkPath(),setting.getPkg().getBaseApkPath()))domains.migrateState(previous,setting,null);
            }
            retained.close();retained=next;applied=true;scheduleWrite();
        }finally{if(!applied)next.close();}
    }
    @Override public synchronized void close(){enforce();closed=true;changes=null;if(retained!=null){retained.close();retained=null;}}
    public void scheduleWrite(){
        synchronized(this){if(!seeded||closed)throw new IllegalStateException("original domain settings not seeded or closed");}
        if(!handler.post(()->{
            final byte[] record;final IPackageDomainSettingsChanged target;
            synchronized(this){if(closed)return;record=captureState();target=Objects.requireNonNull(changes);}
            try{target.changed(record);}catch(RemoteException failure){throw failure.rethrowFromSystemServer();}
        }))throw new IllegalStateException("domain persistence Handler stopped");
    }
}
