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
import android.os.Parcel;
import android.os.ShellCommand;
import android.os.UserHandle;
import java.util.Objects;

/** Original Intent.parseCommandArgs; no native shell syntax fork. */
public final class PackageIntentParser extends IPackageIntentParser.Stub {
    // ActivityManagerShellCommand.makeIntent/runStartActivity, android-16.0.0_r1.
    // Wait/profiler/options paths remain unsupported; Intent parses all intent options.
    @Override public byte[] parseActivityStart(String[] arguments) {
        Bridge.enforceSystemUid();
        final byte[][] result=new byte[1][];
        ShellCommand parser=new ShellCommand(){
            @Override public void onHelp(){}
            @Override public int onCommand(String ignored){
                final int[] user={-2};
                try {
                    Intent intent=Intent.parseCommandArgs(this,(option,command)->{
                        if (!option.equals("--user")) return false;
                        user[0]=UserHandle.parseUserArg(command.getNextArgRequired());return true;
                    });
                    Parcel out=Parcel.obtain();try{
                        out.writeInt(user[0]);
                        out.writeString(intent.toString());
                        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                        intent.writeToParcel(out,0);result[0]=out.marshall();
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
