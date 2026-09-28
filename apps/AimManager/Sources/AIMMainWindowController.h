#import <AppKit/AppKit.h>

@class AIMRuntimeClient;

NS_ASSUME_NONNULL_BEGIN
@interface AIMMainWindowController : NSWindowController
- (instancetype)initWithRuntimeClient:(AIMRuntimeClient *)client;
- (void)installAPKURLs:(NSArray<NSURL *> *)URLs;
@end
NS_ASSUME_NONNULL_END
