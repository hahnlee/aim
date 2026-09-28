#import <AppKit/AppKit.h>

static NSString *const AIMErrorDomain = @"dev.aim.app-shim";

static NSError *AIMMessageError(NSString *message) {
    return [NSError errorWithDomain:AIMErrorDomain
                               code:1
                           userInfo:@{NSLocalizedDescriptionKey: message}];
}

static NSData *AIMRun(NSURL *runtimeRoot, NSString *program,
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
            *error = AIMMessageError(text.length ? text : @"Android 앱을 시작하지 못했습니다.");
        }
        return nil;
    }
    return data;
}

static NSURL *AIMManagerBundleURL(NSDictionary *info) {
    NSString *fallback = info[@"AIMManagerBundlePath"];
    if (fallback.length && [NSFileManager.defaultManager fileExistsAtPath:fallback]) {
        return [NSURL fileURLWithPath:fallback isDirectory:YES];
    }
    return [[NSWorkspace sharedWorkspace]
        URLForApplicationWithBundleIdentifier:@"dev.aim.manager"];
}

static void AIMShowError(NSError *error) {
    [NSApplication sharedApplication];
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"Android 앱을 열 수 없습니다";
    alert.informativeText = error.localizedDescription ?: @"알 수 없는 오류입니다.";
    [alert runModal];
}

int main(void) {
    @autoreleasepool {
        NSDictionary *info = NSBundle.mainBundle.infoDictionary;
        NSString *profile = info[@"AIMProfile"];
        NSString *package = info[@"AIMPackage"];
        NSURL *manager = AIMManagerBundleURL(info);
        NSURL *runtime = [manager URLByAppendingPathComponent:@"Contents/Resources/aim"
                                                  isDirectory:YES];
        NSString *control = [runtime URLByAppendingPathComponent:
                                      @"target/release/aimctl"].path;
        if (!profile.length || !package.length ||
            ![NSFileManager.defaultManager isExecutableFileAtPath:control]) {
            AIMShowError(AIMMessageError(@"aim Manager 또는 앱 연결 정보가 없습니다."));
            return 1;
        }

        NSMutableDictionary<NSString *, NSString *> *environment =
            [NSProcessInfo.processInfo.environment mutableCopy];
        environment[@"AIM_PROFILE"] = profile;
        environment[@"AIM_PACKAGED_RUNTIME"] = @"1";
        environment[@"AIM_HOST_BUNDLE"] =
            [runtime URLByAppendingPathComponent:@"AimHost.app" isDirectory:YES].path;
        environment[@"AIM_ANGLE_DIRECTORY"] =
            [runtime URLByAppendingPathComponent:@"_build/angle-source/out/AimRelease"].path;
        environment[@"AIM_MOLTENVK_DYLIB"] =
            [runtime URLByAppendingPathComponent:@"_build/moltenvk/libMoltenVK.dylib"].path;
        NSString *profilesRoot = environment[@"AIM_PROFILE_ROOT"];
        if (!profilesRoot.length) {
            profilesRoot = [NSHomeDirectory() stringByAppendingPathComponent:
                @"Library/Application Support/aim/profiles"];
        }
        NSString *profileRoot = [profilesRoot stringByAppendingPathComponent:profile];
        environment[@"AIM_NATIVE_CACHE_ROOT"] =
            [profileRoot stringByAppendingPathComponent:@"native-cache"];
        environment[@"AIM_DAEMONIZED_LOG"] =
            [profileRoot stringByAppendingPathComponent:@"managed-apps.log"];
        // The low-level launcher owns setup and daemonizes the final host.
        // Keep the Finder shim short-lived after that ownership handoff.
        environment[@"AIM_ASYNC_LAUNCH"] = @"1";

        NSError *error = nil;
        if (!AIMRun(runtime, control, @[@"ensure"], environment, &error)) {
            AIMShowError(error);
            return 1;
        }
        NSString *launcher = [runtime URLByAppendingPathComponent:
                                       @"tools/run-android-apk-app.sh"].path;
        NSData *result = AIMRun(runtime, launcher, @[@"--package", package, @"86400"],
                                environment, &error);
        if (!result) {
            AIMShowError(error);
            return 1;
        }
    }
    return 0;
}
