# Host device permissions

macOS asks for **AIM Runtime** access when Android uses the Mac's Bluetooth,
camera, microphone or location through `linux-run` (`dev.aim.linux-run`).
Allow the requested access for Android apps to use that device; Android's own
app permissions still apply.

Change a grant in **System Settings → Privacy & Security** under **Bluetooth**,
**Camera**, **Microphone** or **Location Services**, using the **AIM Runtime** entry.
