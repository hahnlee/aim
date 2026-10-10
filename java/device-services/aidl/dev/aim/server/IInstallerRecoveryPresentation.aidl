package dev.aim.server;
/** Original BitmapFactory/Bitmap parcel codec for persisted installer PNGs. */
interface IInstallerRecoveryPresentation {
    byte[] decodeIcon(in byte[] png);
}
