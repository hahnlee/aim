package dev.aim.server;

public final class PackageIntentParserTest {
    public static void main(String[] arguments) throws Exception {
        System.out.write(new PackageIntentParser().parseIntent(arguments));
    }
}
