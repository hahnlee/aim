#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface AIMInstalledApp : NSObject

@property(nonatomic, copy) NSString *packageName;
@property(nonatomic, copy) NSString *displayName;
@property(nonatomic, copy) NSString *version;
@property(nonatomic, nullable) NSData *iconData;

@end

@interface AIMRuntimeSnapshot : NSObject

@property(nonatomic, copy) NSArray<AIMInstalledApp *> *apps;
@property(nonatomic, copy) NSDictionary<NSString *, NSArray<NSNumber *> *> *processes;
@property(nonatomic, copy) NSString *daemonStatus;
@property(nonatomic) unsigned long long allocatedBytes;

@end

typedef void (^AIMRuntimeSnapshotHandler)(AIMRuntimeSnapshot *_Nullable snapshot,
                                           NSError *_Nullable error);
typedef void (^AIMRuntimeActionHandler)(NSError *_Nullable error);
typedef void (^AIMRuntimeLogHandler)(NSString *line);
typedef void (^AIMProfilesHandler)(NSArray<NSString *> *_Nullable profiles,
                                    NSError *_Nullable error);

@interface AIMRuntimeClient : NSObject

@property(nonatomic, readonly) NSURL *runtimeRootURL;
@property(nonatomic, readonly) NSString *profileName;
@property(nonatomic, copy, nullable) AIMRuntimeLogHandler logHandler;

- (nullable instancetype)initWithError:(NSError **)error;
- (void)fetchProfiles:(AIMProfilesHandler)handler;
- (void)switchToProfile:(NSString *)profile;
- (void)createProfile:(NSString *)profile completion:(AIMRuntimeActionHandler)handler;
- (void)deleteProfile:(NSString *)profile completion:(AIMRuntimeActionHandler)handler;
- (void)fetchSnapshot:(AIMRuntimeSnapshotHandler)handler;
- (void)installAPKAtURL:(NSURL *)url completion:(AIMRuntimeActionHandler)handler;
- (void)uninstallPackage:(NSString *)package
              removeData:(BOOL)removeData
              completion:(AIMRuntimeActionHandler)handler;
- (void)launchPackage:(NSString *)package completion:(AIMRuntimeActionHandler)handler;
- (void)stopPackage:(NSString *)package
                pids:(NSArray<NSNumber *> *)pids
          completion:(AIMRuntimeActionHandler)handler;
- (void)revealDataForPackage:(NSString *)package completion:(AIMRuntimeActionHandler)handler;
- (void)restartSystem:(AIMRuntimeActionHandler)handler;
- (void)revealProfile:(AIMRuntimeActionHandler)handler;

@end

NS_ASSUME_NONNULL_END
