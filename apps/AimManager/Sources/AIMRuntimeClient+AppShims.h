#import "AIMRuntimeClient.h"

NS_ASSUME_NONNULL_BEGIN

@interface AIMRuntimeClient (AppShims)

- (void)removeAppShimsForProfile:(NSString *)profile;
- (BOOL)synchronizeAppShims:(NSArray<AIMInstalledApp *> *)apps
                     profile:(NSString *)profile
                       error:(NSError **)error;

@end

NS_ASSUME_NONNULL_END

