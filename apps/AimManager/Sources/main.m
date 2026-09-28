#import <AppKit/AppKit.h>
#import "AIMAppDelegate.h"

int main(void) {
    @autoreleasepool {
        NSApplication *application = NSApplication.sharedApplication;
        AIMAppDelegate *delegate = [[AIMAppDelegate alloc] init];
        application.delegate = delegate;
        [application run];
    }
    return 0;
}
