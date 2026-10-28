package dev.aim.server;

import android.content.Intent;
import android.os.Parcel;
import android.os.ShellCommand;
import android.os.UserHandle;
import java.util.Objects;

/** Original Intent.parseCommandArgs; no native shell syntax fork. */
public final class PackageIntentParser extends IPackageIntentParser.Stub {
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
