/*
 * Copyright (C) 2015 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package dev.aim.server;

import android.content.Intent;
import android.app.ActivityOptions;
import android.os.Parcel;
import android.os.ShellCommand;
import android.os.UserHandle;
import java.util.Objects;

/** Original Intent.parseCommandArgs; no native shell syntax fork. */
public final class PackageIntentParser extends IPackageIntentParser.Stub {
    // ActivityManagerShellCommand.makeIntent/runStartActivity, android-16.0.0_r1.
    // Profiler paths remain unsupported; Intent parses all intent options.
    @Override public byte[] parseActivityStart(String[] arguments) {
        Bridge.enforceSystemUid();
        final byte[][] result=new byte[1][];
        ShellCommand parser=new ShellCommand(){
            @Override public void onHelp(){}
            @Override public int onCommand(String ignored){
                final int[] user={-2};final boolean[] wait={false};
                final int[] launch={-1,-1,0,0,-1};
                final boolean[] flags={false,false,false,false};
                final String[] unsupported={null};
                try {
                    Intent intent=Intent.parseCommandArgs(this,(option,command)->{
                        switch(option){
                            case "-W":wait[0]=true;return true;
                            case "--user":user[0]=UserHandle.parseUserArg(command.getNextArgRequired());return true;
                            case "--display":launch[0]=Integer.parseInt(command.getNextArgRequired());return true;
                            case "--task-display-area-feature-id":launch[1]=Integer.parseInt(command.getNextArgRequired());return true;
                            case "--windowingMode":launch[2]=Integer.parseInt(command.getNextArgRequired());return true;
                            case "--activityType":launch[3]=Integer.parseInt(command.getNextArgRequired());return true;
                            case "--task":launch[4]=Integer.parseInt(command.getNextArgRequired());return true;
                            case "--task-overlay":flags[0]=true;return true;
                            case "--lock-task":flags[1]=true;return true;
                            case "--splashscreen-show-icon":flags[2]=true;return true;
                            case "--dismiss-keyguard":
                            case "--dismiss-keyguard-if-insecure":flags[3]=true;return true;
                            // makeIntent accepts this BroadcastOptions flag; runStartActivity
                            // does not consume the broadcast options.
                            case "--allow-background-activity-starts":return true;
                            case "--allow-fgs-start-reason":Integer.parseInt(command.getNextArgRequired());return true;
                            case "-P":case "--start-profiler":case "--clock-type":
                            case "--attach-agent":case "--attach-agent-bind":case "--receiver-permission":
                                command.getNextArgRequired();unsupported[0]=option;return true;
                            case "--sampling":case "--profiler-output-version":case "-R":
                                Integer.parseInt(command.getNextArgRequired());unsupported[0]=option;return true;
                            case "-D":case "--suspend":case "-N":case "-S":
                            case "--streaming":case "--track-allocation":case "--async":
                                unsupported[0]=option;return true;
                            default:return false;
                        }
                    });
                    if(unsupported[0]!=null || (flags[0] && launch[4]==-1))
                        throw new IllegalArgumentException("Unsupported activity start option: "
                                +(unsupported[0]!=null?unsupported[0]:"--task-overlay")+" (#1485)");
                    ActivityOptions options=null;
                    if(launch[0]!=-1 || launch[1]!=-1 || launch[2]!=0 || launch[3]!=0
                            || launch[4]!=-1 || flags[1] || flags[2] || flags[3]){
                        options=ActivityOptions.makeBasic();
                        if(launch[0]!=-1)options.setLaunchDisplayId(launch[0]);
                        if(launch[1]!=-1)options.setLaunchTaskDisplayAreaFeatureId(launch[1]);
                        if(launch[2]!=0)options.setLaunchWindowingMode(launch[2]);
                        if(launch[3]!=0)options.setLaunchActivityType(launch[3]);
                        if(launch[4]!=-1){
                            options.setLaunchTaskId(launch[4]);
                            if(flags[0])options.setTaskOverlay(true,true);
                        }
                        if(flags[1])options.setLockTaskEnabled(true);
                        if(flags[2])options.setSplashScreenStyle(1); // SPLASH_SCREEN_STYLE_ICON
                        if(flags[3])options.setDismissKeyguardIfInsecure();
                    }
                    Parcel out=Parcel.obtain();try{
                        out.writeInt(user[0]);out.writeInt(wait[0]?1:0);
                        out.writeString(intent.toString());
                        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                        intent.writeToParcel(out,0);
                        out.writeInt(options==null?0:1);
                        if(options!=null)out.writeBundle(options.toBundle());
                        result[0]=out.marshall();
                    }finally{out.recycle();}
                    return 0;
                }catch(java.net.URISyntaxException error){throw new IllegalArgumentException(error.getMessage(),error);}
            }
        };
        parser.init(null,null,null,null,arguments,0);parser.onCommand(null);
        return Objects.requireNonNull(result[0]);
    }


    // ActivityManagerShellCommand.makeIntent/runSendBroadcast, android-16.0.0_r1,
    @Override public byte[] parseBroadcast(String[] arguments) {
        Bridge.enforceSystemUid();
        final byte[][] result=new byte[1][];
        ShellCommand parser=new ShellCommand(){
            @Override public void onHelp(){}
            @Override public int onCommand(String ignored){
                final int[] user={-2};final String[] permission={null};
                try {
                    Intent intent=Intent.parseCommandArgs(this,(option,command)->{
                        switch(option){
                            case "--user":user[0]=UserHandle.parseUserArg(command.getNextArgRequired());return true;
                            case "--receiver-permission":permission[0]=command.getNextArgRequired();return true;
                            default:return false;
                        }
                    });
                    intent.addFlags(0x00400000); // FLAG_RECEIVER_FROM_SHELL
                    Parcel out=Parcel.obtain();try{
                        out.writeInt(user[0]);out.writeString(permission[0]);
                        out.writeString(intent.toString());intent.writeToParcel(out,0);
                        result[0]=out.marshall();
                    }finally{out.recycle();}
                    return 0;
                }catch(java.net.URISyntaxException error){throw new IllegalArgumentException(error.getMessage(),error);}
            }
        };
        parser.init(null,null,null,null,arguments,0);parser.onCommand(null);
        return Objects.requireNonNull(result[0]);
    }

    @Override public String formatBroadcastResult(byte[] result) {
        Bridge.enforceSystemUid();
        Parcel in=Parcel.obtain();try{
            in.unmarshall(result,0,result.length);in.setDataPosition(0);
            String line="Broadcast completed: result="+in.readInt();
            String data=in.readString();android.os.Bundle extras=in.readBundle();
            if(data!=null)line+=", data=\""+data+"\"";
            if(extras!=null)line+=", extras: "+extras;
            return line;
        }finally{in.recycle();}
    }

    @Override public byte[] parseIntent(String[] arguments) {
        Bridge.enforceSystemUid();
        return parse(arguments);
    }

    static byte[] parse(String[] arguments) {
        Objects.requireNonNull(arguments);
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
        parser.init(null,null,null,null,arguments,0);
        parser.onCommand(null);
        return Objects.requireNonNull(result[0]);
    }
}
