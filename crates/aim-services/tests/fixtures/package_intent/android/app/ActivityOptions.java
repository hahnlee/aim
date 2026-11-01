package android.app;

/** Scalar parcel boundary only; production uses the pinned framework class. */
public class ActivityOptions {
    private final android.os.Bundle bundle=new android.os.Bundle();
    public ActivityOptions(){bundle.putInt("android.activity.splashScreenStyle",-1);}
    public static ActivityOptions makeBasic(){return new ActivityOptions();}
    public ActivityOptions setLaunchDisplayId(int value){bundle.putInt("android.activity.launchDisplayId",value);return this;}
    public void setLaunchTaskDisplayAreaFeatureId(int value){bundle.putInt("android.activity.launchTaskDisplayAreaFeatureId",value);}
    public void setLaunchWindowingMode(int value){bundle.putInt("android.activity.windowingMode",value);}
    public void setLaunchActivityType(int value){bundle.putInt("android.activity.activityType",value);}
    public void setLaunchTaskId(int value){bundle.putInt("android.activity.launchTaskId",value);}
    public void setTaskOverlay(boolean overlay,boolean resume){
        bundle.putBoolean("android.activity.taskOverlay",overlay);
        bundle.putBoolean("android.activity.taskOverlayCanResume",resume);
    }
    public ActivityOptions setLockTaskEnabled(boolean value){bundle.putBoolean("android:activity.lockTaskMode",value);return this;}
    public ActivityOptions setSplashScreenStyle(int value){bundle.putInt("android.activity.splashScreenStyle",value);return this;}
    public void setDismissKeyguardIfInsecure(){bundle.putBoolean("android.activity.dismissKeyguardIfInsecure",true);}
    public android.os.Bundle toBundle(){return bundle;}
}
