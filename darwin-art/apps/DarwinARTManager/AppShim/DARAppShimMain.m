#import <AppKit/AppKit.h>

static NSString *const DARErrorDomain = @"dev.darwinart.app-shim";

static NSError *DARMessageError(NSString *message) {
    return [NSError errorWithDomain:DARErrorDomain
                               code:1
                           userInfo:@{NSLocalizedDescriptionKey: message}];
}

static NSData *DARRun(NSURL *runtimeRoot, NSString *program,
                      NSArray<NSString *> *arguments,
                      NSDictionary<NSString *, NSString *> *environment,
                      NSError **error) {
    NSTask *task = [[NSTask alloc] init];
    NSPipe *output = [NSPipe pipe];
    task.executableURL = [NSURL fileURLWithPath:program];
    task.arguments = arguments;
    task.environment = environment;
    task.currentDirectoryURL = runtimeRoot;
    task.standardOutput = output;
    task.standardError = output;
    if (![task launchAndReturnError:error]) return nil;
    NSData *data = [output.fileHandleForReading readDataToEndOfFile];
    [task waitUntilExit];
    if (task.terminationStatus != 0) {
        NSString *text = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"";
        if (error) {
            *error = DARMessageError(text.length ? text : @"Android 앱을 시작하지 못했습니다.");
        }
        return nil;
    }
    return data;
}

static NSURL *DARManagerBundleURL(NSDictionary *info) {
    NSString *fallback = info[@"DARManagerBundlePath"];
    if (fallback.length && [NSFileManager.defaultManager fileExistsAtPath:fallback]) {
        return [NSURL fileURLWithPath:fallback isDirectory:YES];
    }
    return [[NSWorkspace sharedWorkspace]
        URLForApplicationWithBundleIdentifier:@"dev.darwinart.manager"];
}

static void DARShowError(NSError *error) {
    [NSApplication sharedApplication];
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"Android 앱을 열 수 없습니다";
    alert.informativeText = error.localizedDescription ?: @"알 수 없는 오류입니다.";
    [alert runModal];
}

int main(void) {
    @autoreleasepool {
        NSDictionary *info = NSBundle.mainBundle.infoDictionary;
        NSString *profile = info[@"DARProfile"];
        NSString *package = info[@"DARPackage"];
        NSURL *manager = DARManagerBundleURL(info);
        NSURL *runtime = [manager URLByAppendingPathComponent:@"Contents/Resources/DarwinART"
                                                  isDirectory:YES];
        NSString *control = [runtime URLByAppendingPathComponent:
                                      @"target/release/darwin-artctl"].path;
        if (!profile.length || !package.length ||
            ![NSFileManager.defaultManager isExecutableFileAtPath:control]) {
            DARShowError(DARMessageError(@"Darwin ART Manager 또는 앱 연결 정보가 없습니다."));
            return 1;
        }

        NSMutableDictionary<NSString *, NSString *> *environment =
            [NSProcessInfo.processInfo.environment mutableCopy];
        environment[@"DARWIN_ART_PROFILE"] = profile;
        environment[@"DARWIN_ART_PACKAGED_RUNTIME"] = @"1";
        environment[@"DARWIN_ART_HOST_BUNDLE"] =
            [runtime URLByAppendingPathComponent:@"DarwinARTHost.app" isDirectory:YES].path;
        environment[@"DARWIN_ART_ANGLE_DIRECTORY"] =
            [runtime URLByAppendingPathComponent:@"_build/angle-source/out/DarwinArtRelease"].path;
        environment[@"DARWIN_ART_MOLTENVK_DYLIB"] =
            [runtime URLByAppendingPathComponent:@"_build/moltenvk/libMoltenVK.dylib"].path;
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
        // The low-level launcher owns setup and daemonizes the final host.
        // Keep the Finder shim short-lived after that ownership handoff.
        environment[@"DARWIN_ART_ASYNC_LAUNCH"] = @"1";

        NSError *error = nil;
        if (!DARRun(runtime, control, @[@"ensure"], environment, &error)) {
            DARShowError(error);
            return 1;
        }
        NSData *record = DARRun(runtime, control, @[@"resolve", package], environment, &error);
        if (!record) {
            DARShowError(error);
            return 1;
        }
        NSURL *recordURL = [[NSURL fileURLWithPath:NSTemporaryDirectory() isDirectory:YES]
            URLByAppendingPathComponent:[NSString stringWithFormat:@"darwin-art-shim-%@.record",
                                                                   NSUUID.UUID.UUIDString]];
        if (![record writeToURL:recordURL options:NSDataWritingAtomic error:&error]) {
            DARShowError(error);
            return 1;
        }
        NSString *launcher = [runtime URLByAppendingPathComponent:
                                       @"tools/run-android-apk-app.sh"].path;
        NSData *result = DARRun(runtime, launcher,
                                @[@"--record", recordURL.path, @"86400"], environment, &error);
        if (!result) {
            [NSFileManager.defaultManager removeItemAtURL:recordURL error:nil];
            DARShowError(error);
            return 1;
        }
        [NSFileManager.defaultManager removeItemAtURL:recordURL error:nil];
    }
    return 0;
}
