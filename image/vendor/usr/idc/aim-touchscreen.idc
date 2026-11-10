# The display server's window as the device's touchscreen (docs/input.md).
# Its axes are the display's pixels, so it maps 1:1 onto the internal
# display (display 0).

device.internal = 1

touch.deviceType = touchScreen
touch.orientationAware = 1
# A click in a window wakes the device, as the Mac's own input wakes the
# Mac (TouchInputMapper's POLICY_FLAG_WAKE; the window policy then wakes
# it, WAKE_REASON_WAKE_MOTION).
touch.wake = 1
