#import "DARRuntimeClient.h"
#import "DARRuntimeClient+AppShims.h"

#import <AppKit/AppKit.h>

static NSString *const DARErrorDomain = @"dev.darwinart.manager";

@implementation DARInstalledApp
@end

@implementation DARRuntimeSnapshot
@end

@interface DARRuntimeClient ()
@property(nonatomic, readwrite) NSURL *runtimeRootURL;
@property(nonatomic, readwrite) NSString *profileName;
@end

@implementation DARRuntimeClient

- (nullable instancetype)initWithError:(NSError **)error {
    self = [super init];
    if (!self) return nil;

    NSURL *root = [self discoverRuntimeRoot];
    if (!root) {
        if (error) {
            *error = [NSError errorWithDomain:DARErrorDomain
                                         code:1
                                     userInfo:@{NSLocalizedDescriptionKey:
                                                    @"앱에 포함된 Darwin ART 런타임이 없거나 손상되었습니다."}];
        }
        return nil;
    }
    _runtimeRootURL = root;
    _profileName = NSProcessInfo.processInfo.environment[@"DARWIN_ART_PROFILE"] ?: @"default";
    return self;
}

- (NSURL *)discoverRuntimeRoot {
    NSFileManager *files = NSFileManager.defaultManager;
    NSString *override = NSProcessInfo.processInfo.environment[@"DARWIN_ART_RUNTIME_ROOT"];
    if (override.length > 0) {
        NSURL *candidate = [NSURL fileURLWithPath:override isDirectory:YES];
        if ([files isExecutableFileAtPath:
                       [candidate URLByAppendingPathComponent:@"target/release/darwin-artctl"].path]) {
            return candidate;
        }
    }
    NSURL *candidate = [NSBundle.mainBundle.resourceURL URLByAppendingPathComponent:@"DarwinART"
                                                                         isDirectory:YES];
    BOOL hasControl = [files isExecutableFileAtPath:
                                 [candidate URLByAppendingPathComponent:
                                                @"target/release/darwin-artctl"].path];
    BOOL hasRuntime = [files fileExistsAtPath:
                                 [candidate URLByAppendingPathComponent:
                                                @"_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib"].path];
    if (hasControl && hasRuntime) return candidate;
    return nil;
}

- (NSMutableDictionary<NSString *, NSString *> *)taskEnvironment {
    NSMutableDictionary *environment = [NSProcessInfo.processInfo.environment mutableCopy];
    NSString *profile = nil;
    @synchronized(self) {
        profile = self.profileName;
    }
    environment[@"DARWIN_ART_PROFILE"] = profile;
    environment[@"DARWIN_ART_PACKAGED_RUNTIME"] = @"1";
    environment[@"DARWIN_ART_HOST_BUNDLE"] =
        [self.runtimeRootURL URLByAppendingPathComponent:@"DarwinARTHost.app" isDirectory:YES].path;
    environment[@"DARWIN_ART_ANGLE_DIRECTORY"] =
        [self.runtimeRootURL URLByAppendingPathComponent:
                                 @"_build/angle-source/out/DarwinArtRelease"].path;
    environment[@"DARWIN_ART_MOLTENVK_DYLIB"] =
        [self.runtimeRootURL URLByAppendingPathComponent:
                                 @"_build/moltenvk/libMoltenVK.dylib"].path;
    NSString *profilesRoot = environment[@"DARWIN_ART_PROFILE_ROOT"];
    if (!profilesRoot.length) {
        profilesRoot = [NSHomeDirectory() stringByAppendingPathComponent:
            @"Library/Application Support/DarwinART/profiles"];
    }
    NSString *profileRoot = [profilesRoot stringByAppendingPathComponent:profile];
    environment[@"DARWIN_ART_NATIVE_CACHE_ROOT"] =
        [profileRoot stringByAppendingPathComponent:@"native-cache"];
    environment[@"DARWIN_ART_DAEMONIZED_LOG"] =
        [profileRoot stringByAppendingPathComponent:@"managed-apps.log"];
    return environment;
}

- (NSString *)controlProgram {
    return [self.runtimeRootURL URLByAppendingPathComponent:
                                     @"target/release/darwin-artctl"].path;
}

- (nullable NSData *)runDataProgram:(NSString *)program
                          arguments:(NSArray<NSString *> *)arguments
                              error:(NSError **)error {
    NSTask *task = [[NSTask alloc] init];
    NSPipe *output = [NSPipe pipe];
    task.executableURL = [NSURL fileURLWithPath:program];
    task.arguments = arguments;
    task.environment = [self taskEnvironment];
    task.currentDirectoryURL = self.runtimeRootURL;
    task.standardOutput = output;
    task.standardError = output;
    if (![task launchAndReturnError:error]) return nil;
    NSData *data = [output.fileHandleForReading readDataToEndOfFile];
    [task waitUntilExit];
    if (task.terminationStatus != 0) {
        if (error) {
            NSString *text = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"";
            NSString *detail = [text stringByTrimmingCharactersInSet:
                                         NSCharacterSet.whitespaceAndNewlineCharacterSet];
            *error = [NSError errorWithDomain:DARErrorDomain
                                         code:task.terminationStatus
                                     userInfo:@{NSLocalizedDescriptionKey:
                                                    detail.length ? detail : @"명령 실행에 실패했습니다."}];
        }
        return nil;
    }
    return data;
}

- (NSString *)runProgram:(NSString *)program
                arguments:(NSArray<NSString *> *)arguments
                     error:(NSError **)error {
    NSData *data = [self runDataProgram:program arguments:arguments error:error];
    return data ? ([[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"") : @"";
}

- (void)fetchProfiles:(DARProfilesHandler)handler {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        NSString *text = [self runProgram:self.controlProgram arguments:@[@"profiles"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(nil, error); });
        NSMutableArray<NSString *> *profiles = [NSMutableArray array];
        [text enumerateLinesUsingBlock:^(NSString *line, BOOL *stop) {
            (void)stop;
            if (line.length) [profiles addObject:line];
        }];
        dispatch_async(dispatch_get_main_queue(), ^{ handler(profiles, nil); });
    });
}

- (void)switchToProfile:(NSString *)profile {
    @synchronized(self) {
        self.profileName = [profile copy];
    }
}

- (void)createProfile:(NSString *)profile completion:(DARRuntimeActionHandler)handler {
    [self runActionProgram:self.controlProgram
                 arguments:@[@"create-profile", profile]
                completion:handler];
}

- (void)deleteProfile:(NSString *)profile completion:(DARRuntimeActionHandler)handler {
    [self runActionProgram:self.controlProgram
                 arguments:@[@"delete-profile", profile]
                completion:^(NSError *error) {
                    if (!error) [self removeAppShimsForProfile:profile];
                    handler(error);
                }];
}

// The launcher's `--launcher-info` blocks: PackageManager's label, version
// and icon for each installed package, keyed by package.
- (NSDictionary<NSString *, NSDictionary<NSString *, NSString *> *> *)launcherInfo:
        (NSArray<NSString *> *)packages {
    if (packages.count == 0) return @{};
    NSString *launcher = [self.runtimeRootURL URLByAppendingPathComponent:
                                                   @"tools/run-android-apk-app.sh"].path;
    NSError *error = nil;
    NSString *text = [self runProgram:launcher
                            arguments:[@[@"--launcher-info"] arrayByAddingObjectsFromArray:packages]
                                error:&error];
    if (error) return @{};
    NSMutableDictionary *result = [NSMutableDictionary dictionary];
    __block NSMutableDictionary<NSString *, NSString *> *current = nil;
    [text enumerateLinesUsingBlock:^(NSString *line, BOOL *stop) {
        (void)stop;
        NSRange equals = [line rangeOfString:@"="];
        if (equals.location == NSNotFound) {
            current = nil;
            return;
        }
        NSString *key = [line substringToIndex:equals.location];
        NSString *value = [line substringFromIndex:NSMaxRange(equals)];
        if ([key isEqualToString:@"package"]) {
            current = [NSMutableDictionary dictionary];
            result[value] = current;
        }
        current[key] = value;
    }];
    return result;
}

- (DARInstalledApp *)installedAppForPackage:(NSString *)package
                                       info:(NSDictionary<NSString *, NSString *> *)info {
    DARInstalledApp *app = [[DARInstalledApp alloc] init];
    app.packageName = package;
    app.displayName = info[@"label"].length ? info[@"label"] : package;
    app.version = info[@"versionCode"] ?: @"";
    NSString *icon = info[@"icon"];
    if (icon.length) {
        NSData *data = [NSData dataWithContentsOfFile:icon];
        if (data.length && [[NSImage alloc] initWithData:data]) app.iconData = data;
    }
    return app;
}

- (void)fetchSnapshot:(DARRuntimeSnapshotHandler)handler {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        NSString *ctl = self.controlProgram;
        [self runProgram:ctl arguments:@[@"ensure"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(nil, error); });
        NSString *packagesText = [self runProgram:ctl arguments:@[@"list"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(nil, error); });
        NSString *processText = [self runProgram:ctl arguments:@[@"ps"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(nil, error); });
        NSString *status = [self runProgram:ctl arguments:@[@"status"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(nil, error); });

        NSMutableArray<NSString *> *packages = [NSMutableArray array];
        [packagesText enumerateLinesUsingBlock:^(NSString *line, BOOL *stop) {
            (void)stop;
            if (line.length) [packages addObject:line];
        }];
        NSDictionary *infos = [self launcherInfo:packages];
        NSMutableArray<DARInstalledApp *> *apps = [NSMutableArray array];
        for (NSString *package in packages) {
            [apps addObject:[self installedAppForPackage:package info:infos[package] ?: @{}]];
        }
        NSMutableDictionary<NSString *, NSMutableArray<NSNumber *> *> *processes =
            [NSMutableDictionary dictionary];
        [processText enumerateLinesUsingBlock:^(NSString *line, BOOL *stop) {
            (void)stop;
            NSArray<NSString *> *fields = [line componentsSeparatedByString:@"\t"];
            if (fields.count != 2) return;
            NSInteger pid = fields[0].integerValue;
            if (pid <= 0) return;
            NSMutableArray *values = processes[fields[1]];
            if (!values) processes[fields[1]] = values = [NSMutableArray array];
            [values addObject:@(pid)];
        }];
        DARRuntimeSnapshot *snapshot = [[DARRuntimeSnapshot alloc] init];
        snapshot.apps = apps;
        snapshot.processes = processes;
        snapshot.daemonStatus = [status stringByTrimmingCharactersInSet:
                                            NSCharacterSet.whitespaceAndNewlineCharacterSet];
        NSString *size = [self runProgram:ctl
                                arguments:@[@"profile-size", self.profileName]
                                    error:&error];
        snapshot.allocatedBytes = error ? 0 : strtoull(size.UTF8String, NULL, 10);
        NSError *shimError = nil;
        NSString *profile = nil;
        @synchronized(self) { profile = self.profileName; }
        if (![self synchronizeAppShims:apps profile:profile error:&shimError] && self.logHandler) {
            NSString *warning = [NSString stringWithFormat:@"macOS 앱 동기화 실패: %@\n",
                                                         shimError.localizedDescription];
            NSLog(@"%@", [warning stringByTrimmingCharactersInSet:
                                      NSCharacterSet.whitespaceAndNewlineCharacterSet]);
            dispatch_async(dispatch_get_main_queue(), ^{ self.logHandler(warning); });
        }
        dispatch_async(dispatch_get_main_queue(), ^{ handler(snapshot, nil); });
    });
}

- (void)runActionProgram:(NSString *)program
                arguments:(NSArray<NSString *> *)arguments
                completion:(DARRuntimeActionHandler)handler {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        NSString *output = [self runProgram:program arguments:arguments error:&error];
        if (output.length && self.logHandler) {
            dispatch_async(dispatch_get_main_queue(), ^{ self.logHandler(output); });
        }
        dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
    });
}

- (void)installAPKAtURL:(NSURL *)url completion:(DARRuntimeActionHandler)handler {
    NSString *installer = [self.runtimeRootURL URLByAppendingPathComponent:
                                                     @"tools/run-android-apk-app.sh"].path;
    // A PackageInstaller session in the profile's PackageManagerService.
    [self runActionProgram:installer arguments:@[@"--install", url.path] completion:handler];
}

- (void)uninstallPackage:(NSString *)package
              removeData:(BOOL)removeData
              completion:(DARRuntimeActionHandler)handler {
    NSString *ctl = self.controlProgram;
    NSArray<NSString *> *arguments = removeData
                                         ? @[@"uninstall", package]
                                         : @[@"uninstall", package, @"--keep-data"];
    [self runActionProgram:ctl arguments:arguments completion:handler];
}

- (void)launchPackage:(NSString *)package completion:(DARRuntimeActionHandler)handler {
    NSString *launcher = [self.runtimeRootURL URLByAppendingPathComponent:
                                                   @"tools/run-android-apk-app.sh"].path;
    // The launcher daemonizes the final host and waits synchronously by
    // default. The manager asks only for an asynchronous handoff so the
    // manager task does not own or wait on the app window.
    [self runActionProgram:@"/usr/bin/env"
                 arguments:@[@"DARWIN_ART_ASYNC_LAUNCH=1", launcher, @"--package", package,
                             @"86400"]
                completion:handler];
}

- (void)stopPackage:(NSString *)package
                pids:(NSArray<NSNumber *> *)pids
          completion:(DARRuntimeActionHandler)handler {
    if (pids.count == 0) {
        handler([NSError errorWithDomain:DARErrorDomain
                                    code:2
                                userInfo:@{NSLocalizedDescriptionKey: @"실행 중인 프로세스가 없습니다."}]);
        return;
    }
    NSMutableArray<NSString *> *arguments = [NSMutableArray arrayWithObject:@"-TERM"];
    for (NSNumber *pid in pids) [arguments addObject:pid.stringValue];
    if (self.logHandler) {
        NSString *pidList = [[pids valueForKey:@"stringValue"] componentsJoinedByString:@", "];
        self.logHandler([NSString stringWithFormat:@"%@에 정상 종료 요청 전달 (PID %@)\n",
                                                   package, pidList]);
    }
    [self runActionProgram:@"/bin/kill" arguments:arguments completion:handler];
}

- (void)revealDataForPackage:(NSString *)package completion:(DARRuntimeActionHandler)handler {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        NSString *ctl = self.controlProgram;
        NSString *mount = [self runProgram:ctl arguments:@[@"ensure"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
        mount = [mount stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        NSString *path = [mount stringByAppendingPathComponent:
                                   [NSString stringWithFormat:@"data/apps/%@/private-data/user/0/%@",
                                                              package, package]];
        dispatch_async(dispatch_get_main_queue(), ^{
            [[NSWorkspace sharedWorkspace] selectFile:nil inFileViewerRootedAtPath:path];
            handler(nil);
        });
    });
}

- (void)restartSystem:(DARRuntimeActionHandler)handler {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        NSString *ctl = self.controlProgram;
        NSString *processText = [self runProgram:ctl arguments:@[@"ps"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
        NSMutableArray<NSString *> *pids = [NSMutableArray arrayWithObject:@"-TERM"];
        [processText enumerateLinesUsingBlock:^(NSString *line, BOOL *stop) {
            (void)stop;
            NSString *pid = [[line componentsSeparatedByString:@"\t"] firstObject];
            if (pid.integerValue > 0) [pids addObject:pid];
        }];
        if (pids.count > 1) [self runProgram:@"/bin/kill" arguments:pids error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
        for (NSUInteger attempt = 0; attempt < 50; attempt++) {
            [NSThread sleepForTimeInterval:0.1];
            NSString *remaining = [self runProgram:ctl arguments:@[@"ps"] error:&error];
            if (error || remaining.length == 0) break;
        }
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
        [self runProgram:ctl arguments:@[@"shutdown"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
        [self runProgram:ctl arguments:@[@"ensure"] error:&error];
        dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
    });
}

- (void)revealProfile:(DARRuntimeActionHandler)handler {
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *error = nil;
        NSString *ctl = self.controlProgram;
        NSString *mount = [self runProgram:ctl arguments:@[@"ensure"] error:&error];
        if (error) return dispatch_async(dispatch_get_main_queue(), ^{ handler(error); });
        mount = [mount stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
        dispatch_async(dispatch_get_main_queue(), ^{
            [[NSWorkspace sharedWorkspace] selectFile:nil inFileViewerRootedAtPath:mount];
            handler(nil);
        });
    });
}

@end
