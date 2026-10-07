import android.content.pm.PackageInstaller.SessionParams;
import android.content.pm.PackageInstaller.SessionInfo;
import android.os.Parcel;
import android.graphics.Bitmap;
/** Executes the original framework constructors and CREATORs, never test replacements. */
public final class InstallerCodecOracle {
static SessionParams params(int mode){var value=new SessionParams(0);
value.mode=mode==0?0:11;
value.installFlags=mode==0?0:12;
value.installLocation=mode==0?0:13;
value.installReason=mode==0?0:14;
value.installScenario=mode==0?0:15;
value.sizeBytes=mode==0?0:0x100000000L+16;
value.appPackageName=mode==0?null:"app_package_name 😀";
value.appIcon=icon(mode);
value.appLabel=mode==0?null:"app_label 😀";
value.originatingUri=uri(mode);
value.originatingUid=mode==0?0:21;
value.referrerUri=uri(mode);
value.abiOverride=mode==0?null:"abi_override 😀";
value.volumeUuid=mode==0?null:"volume_uuid 😀";
if(mode!=0){value.setPermissionState("BB",1);value.setPermissionState("Aa",2);value.setPermissionState("BB",2);}
value.whitelistedRestrictedPermissions=mode==0?null:java.util.Arrays.asList("BB",null,"Aa");
value.autoRevokePermissionsMode=mode==0?0:27;
value.installerPackageName=mode==0?null:"installer_package_name 😀";
value.isMultiPackage=mode!=0 && true;
value.isStaged=mode!=0 && false;
value.forceQueryableOverride=mode!=0 && true;
value.requiredInstalledVersionCode=mode==0?0:0x100000000L+32;
value.dataLoaderParams=mode==3?android.content.pm.DataLoaderParams.forStreaming(new android.content.ComponentName("loader.package","Loader"),"--arg 😀"):null;
value.rollbackDataPolicy=mode==0?0:34;
value.rollbackLifetimeMillis=mode==0?0:0x100000000L+35;
value.rollbackImpactLevel=mode==0?0:36;
value.requireUserAction=mode==0?0:37;
value.packageSource=mode==0?0:38;
value.applicationEnabledSettingPersistent=mode!=0 && true;
value.developmentInstallFlags=mode==0?0:40;
value.unarchiveId=mode==0?0:41;
value.dexoptCompilerFilter=mode==0?null:"dexopt_compiler_filter 😀";
value.isAutoInstallDependenciesEnabled=mode!=0 && true;
return value;}
static SessionInfo info(int mode){var value=new SessionInfo();
value.sessionId=mode==0?0:11;
value.userId=mode==0?0:12;
value.installerPackageName=mode==0?null:"installer_package_name 😀";
value.installerAttributionTag=mode==0?null:"installer_attribution_tag 😀";
value.resolvedBaseCodePath=mode==0?null:"resolved_base_code_path 😀";
value.progress=mode==0?0f:0.375f;
value.sealed=mode!=0 && true;
value.active=mode!=0 && false;
value.mode=mode==0?0:19;
value.installReason=mode==0?0:20;
value.installScenario=mode==0?0:21;
value.sizeBytes=mode==0?0:0x100000000L+22;
value.appPackageName=mode==0?null:"app_package_name 😀";
value.appIcon=icon(mode);
value.appLabel=mode==0?null:"app_label 😀";
value.installLocation=mode==0?0:26;
value.originatingUri=uri(mode);
value.originatingUid=mode==0?0:28;
value.referrerUri=uri(mode);
value.grantedRuntimePermissions=mode==0?null:new String[]{"BB",null,"Aa"};
value.whitelistedRestrictedPermissions=mode==0?null:java.util.Arrays.asList("BB",null,"Aa");
value.autoRevokePermissionsMode=mode==0?0:32;
value.installFlags=mode==0?0:33;
value.isMultiPackage=mode!=0 && false;
value.isStaged=mode!=0 && true;
value.forceQueryable=mode!=0 && false;
value.parentSessionId=mode==0?0:37;
value.childSessionIds=mode==0?new int[0]:new int[]{37,99};
value.isSessionApplied=mode!=0 && true;
value.isSessionReady=mode!=0 && false;
value.isSessionFailed=mode!=0 && true;
value.setSessionErrorCode(mode==0?0:42,mode==0?null:"session_error_message 😀");
value.isCommitted=mode!=0 && false;
value.isPreapprovalRequested=mode!=0 && true;
value.rollbackDataPolicy=mode==0?0:46;
value.rollbackLifetimeMillis=mode==0?0:0x100000000L+47;
value.rollbackImpactLevel=mode==0?0:48;
value.createdMillis=mode==0?0:0x100000000L+49;
value.requireUserAction=mode==0?0:50;
value.installerUid=mode==0?0:51;
value.packageSource=mode==0?0:52;
value.applicationEnabledSettingPersistent=mode!=0 && true;
value.pendingUserActionReason=mode==0?0:54;
value.isAutoInstallingDependenciesEnabled=mode!=0 && true;
return value;}
static Bitmap icon(int mode){if(mode!=2&&mode!=4)return null;var icon=Bitmap.createBitmap(1,1,Bitmap.Config.ARGB_8888);icon.setDensity(160);icon.setPixel(0,0,0xff123456);if(mode==4){var gainmap=new android.graphics.Gainmap(icon(2));gainmap.setRatioMin(.5f,.75f,1f);gainmap.setRatioMax(2f,3f,4f);gainmap.setGamma(.8f,1f,1.2f);gainmap.setEpsilonSdr(.001f,.002f,.003f);gainmap.setEpsilonHdr(.004f,.005f,.006f);gainmap.setMinDisplayRatioForHdrTransition(1.2f);gainmap.setDisplayRatioForFullHdr(4f);gainmap.setGainmapDirection(0);icon.setGainmap(gainmap);}return icon;}
static android.net.Uri uri(int mode){return mode==4?new android.net.Uri.Builder().scheme("https").authority("hierarchy.test").path("/a b").query("x=y").fragment("z").build():mode<2?null:mode==2?android.net.Uri.parse("https://example.test/a?b=c#d"):android.net.Uri.fromParts("scheme","body","fragment");}
static byte[] bytes(android.os.Parcelable value){var p=Parcel.obtain();try{value.writeToParcel(p,0);if(p.hasFileDescriptors())throw new AssertionError("fixture unexpectedly has FDs");return p.marshall();}finally{p.recycle();}}
static void iconEquals(Bitmap actual,Bitmap expected){if((actual==null)!=(expected==null))throw new AssertionError("icon nullability");if(actual!=null&&(actual.getWidth()!=1||actual.getHeight()!=1||actual.getPixel(0,0)!=expected.getPixel(0,0)))throw new AssertionError("icon pixels");if(actual!=null){if(actual.hasGainmap()!=expected.hasGainmap())throw new AssertionError("gainmap presence");if(actual.hasGainmap()){var a=actual.getGainmap();var e=expected.getGainmap();if(a.getGainmapContents().getPixel(0,0)!=e.getGainmapContents().getPixel(0,0)||!java.util.Arrays.equals(a.getRatioMin(),e.getRatioMin())||!java.util.Arrays.equals(a.getRatioMax(),e.getRatioMax())||!java.util.Arrays.equals(a.getGamma(),e.getGamma())||!java.util.Arrays.equals(a.getEpsilonSdr(),e.getEpsilonSdr())||!java.util.Arrays.equals(a.getEpsilonHdr(),e.getEpsilonHdr())||a.getDisplayRatioForFullHdr()!=e.getDisplayRatioForFullHdr()||a.getMinDisplayRatioForHdrTransition()!=e.getMinDisplayRatioForHdrTransition()||a.getGainmapDirection()!=e.getGainmapDirection())throw new AssertionError("gainmap fields");}}}
static Parcel load(java.io.File file)throws Exception{var data=java.nio.file.Files.readAllBytes(file.toPath());var p=Parcel.obtain();p.unmarshall(data,0,data.length);p.setDataPosition(0);return p;}
public static void main(String[] args)throws Exception{var dir=new java.io.File(args[0]);for(int mode=0;mode<5;mode++){
var expected=params(mode);java.nio.file.Files.write(new java.io.File(dir,"params-"+mode+".original").toPath(),bytes(expected));
var p=load(new java.io.File(dir,"params-"+mode+".native"));try{var actual=SessionParams.CREATOR.createFromParcel(p);if(p.dataAvail()!=0)throw new AssertionError("params tail");iconEquals(actual.appIcon,expected.appIcon);actual.appIcon=null;expected.appIcon=null;if(!java.util.Arrays.equals(bytes(actual),bytes(expected)))throw new AssertionError("params fields differ "+mode+" actual URI="+actual.originatingUri+" expected URI="+expected.originatingUri);}finally{p.recycle();}
var infoExpected=info(mode);java.nio.file.Files.write(new java.io.File(dir,"info-"+mode+".original").toPath(),bytes(infoExpected));
p=load(new java.io.File(dir,"info-"+mode+".native"));try{var actual=SessionInfo.CREATOR.createFromParcel(p);if(p.dataAvail()!=0)throw new AssertionError("info tail");iconEquals(actual.appIcon,infoExpected.appIcon);actual.appIcon=null;infoExpected.appIcon=null;if(!java.util.Arrays.equals(bytes(actual),bytes(infoExpected)))throw new AssertionError("info fields differ "+mode+" actual URI="+actual.originatingUri+" expected URI="+infoExpected.originatingUri);}finally{p.recycle();}
}System.out.println("original installer codec differential passed");}}
