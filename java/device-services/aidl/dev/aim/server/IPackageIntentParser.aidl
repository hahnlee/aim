package dev.aim.server;
/** Original framework shell syntax, called only by the native system owner. */
interface IPackageIntentParser {
    byte[] parseIntent(in String[] arguments);
    byte[] parseBroadcast(in String[] arguments);
    String formatBroadcastResult(in byte[] result);
    byte[] parseActivityStart(in String[] arguments);
}
