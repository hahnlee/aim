#import "AIMAppDelegate.h"

#import "AIMMainWindowController.h"
#import "AIMRuntimeClient.h"

@interface AIMAppDelegate ()
@property(nonatomic) AIMMainWindowController *mainWindowController;
@property(nonatomic) NSMutableArray<NSURL *> *pendingAPKURLs;
@end

@implementation AIMAppDelegate

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
    [self installApplicationIcon];
    NSError *error = nil;
    AIMRuntimeClient *client = [[AIMRuntimeClient alloc] initWithError:&error];
    if (!client) {
        [[NSAlert alertWithError:error] runModal];
        [NSApp terminate:nil];
        return;
    }
    self.mainWindowController = [[AIMMainWindowController alloc] initWithRuntimeClient:client];
    [self.mainWindowController showWindow:nil];
    if (self.pendingAPKURLs.count) {
        [self.mainWindowController installAPKURLs:self.pendingAPKURLs];
        [self.pendingAPKURLs removeAllObjects];
    }
    [NSApp activateIgnoringOtherApps:YES];
}

- (void)application:(NSApplication *)application openURLs:(NSArray<NSURL *> *)URLs {
    NSMutableArray<NSURL *> *APKs = [NSMutableArray array];
    for (NSURL *URL in URLs) {
        if (URL.isFileURL && [URL.pathExtension.lowercaseString isEqualToString:@"apk"]) {
            [APKs addObject:URL];
        }
    }
    if (!APKs.count) return;
    if (self.mainWindowController) {
        [self.mainWindowController installAPKURLs:APKs];
        [self.mainWindowController showWindow:nil];
        [NSApp activateIgnoringOtherApps:YES];
    } else {
        if (!self.pendingAPKURLs) self.pendingAPKURLs = [NSMutableArray array];
        [self.pendingAPKURLs addObjectsFromArray:APKs];
    }
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

- (void)installApplicationIcon {
    NSImage *icon = [[NSImage alloc] initWithSize:NSMakeSize(256, 256)];
    [icon lockFocus];
    NSBezierPath *background = [NSBezierPath bezierPathWithRoundedRect:NSMakeRect(16, 16, 224, 224)
                                                               xRadius:50
                                                               yRadius:50];
    [[NSColor colorWithCalibratedRed:0.20 green:0.42 blue:0.96 alpha:1.0] setFill];
    [background fill];
    NSImage *symbol = [NSImage imageWithSystemSymbolName:@"apps.iphone.fill"
                                accessibilityDescription:@"aim"];
    symbol = [symbol imageWithSymbolConfiguration:
                         [NSImageSymbolConfiguration configurationWithPointSize:118
                                                                         weight:NSFontWeightMedium]];
    symbol.template = YES;
    [NSColor.whiteColor set];
    [symbol drawInRect:NSMakeRect(60, 60, 136, 136)
              fromRect:NSZeroRect
             operation:NSCompositingOperationSourceOver
              fraction:1.0];
    [icon unlockFocus];
    NSApp.applicationIconImage = icon;
}

@end
