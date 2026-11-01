package dev.aim.server;

public final class PackageIntentParserTest {
    public static void main(String[] arguments) throws Exception {
        if (arguments.length>0 && arguments[0].equals("--start")) {
            System.out.write(new PackageIntentParser().parseActivityStart(java.util.Arrays.copyOfRange(arguments,1,arguments.length)));return;
        }
        System.out.write(arguments.length>0 && arguments[0].equals("--broadcast")
                ? new PackageIntentParser().parseBroadcast(java.util.Arrays.copyOfRange(arguments,1,arguments.length))
                : new PackageIntentParser().parseIntent(arguments));
    }
}
