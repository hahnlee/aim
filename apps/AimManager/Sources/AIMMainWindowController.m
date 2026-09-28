#import "AIMMainWindowController.h"

#import "AIMRuntimeClient.h"
#import <ServiceManagement/ServiceManagement.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

static NSToolbarItemIdentifier const AIMInstallItem = @"dev.aim.manager.install";
static NSToolbarItemIdentifier const AIMRefreshItem = @"dev.aim.manager.refresh";
static NSToolbarItemIdentifier const AIMSystemItem = @"dev.aim.manager.system";

@interface AIMMainWindowController () <NSTableViewDataSource, NSTableViewDelegate,
                                        NSToolbarDelegate, NSDraggingDestination>
@property(nonatomic) AIMRuntimeClient *client;
@property(nonatomic) AIMRuntimeSnapshot *snapshot;
@property(nonatomic) NSArray<AIMInstalledApp *> *apps;
@property(nonatomic) NSArray<NSString *> *profiles;
@property(nonatomic) NSString *selectedPackage;
@property(nonatomic) NSPopUpButton *profilePopup;
@property(nonatomic) NSButton *removeProfileButton;
@property(nonatomic) NSTableView *tableView;
@property(nonatomic) NSImageView *appIconView;
@property(nonatomic) NSTextField *packageLabel;
@property(nonatomic) NSTextField *stateLabel;
@property(nonatomic) NSTextField *daemonLabel;
@property(nonatomic) NSButton *runButton;
@property(nonatomic) NSButton *stopButton;
@property(nonatomic) NSButton *dataButton;
@property(nonatomic) NSButton *deleteButton;
@property(nonatomic) NSTextView *logView;
@property(nonatomic) NSProgressIndicator *progress;
@property(nonatomic) NSMutableString *pendingLogText;
@end

@implementation AIMMainWindowController

- (instancetype)initWithRuntimeClient:(AIMRuntimeClient *)client {
    NSWindow *window = [[NSWindow alloc]
        initWithContentRect:NSMakeRect(0, 0, 940, 620)
                  styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                            NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable |
                            NSWindowStyleMaskFullSizeContentView
                    backing:NSBackingStoreBuffered
                      defer:NO];
    self = [super initWithWindow:window];
    if (!self) return nil;
    _client = client;
    _apps = @[];
    _profiles = @[];
    _pendingLogText = [NSMutableString string];
    window.title = @"aim";
    window.subtitle = [NSString stringWithFormat:@"%@ 프로필", client.profileName];
    window.minSize = NSMakeSize(760, 500);
    window.titlebarAppearsTransparent = YES;
    window.toolbarStyle = NSWindowToolbarStyleUnified;
    window.tabbingMode = NSWindowTabbingModeDisallowed;
    [window center];
    [self buildInterface];
    [window registerForDraggedTypes:@[NSPasteboardTypeFileURL]];
    [self configureToolbar];
    __weak typeof(self) weakSelf = self;
    client.logHandler = ^(NSString *text) { [weakSelf appendLog:text]; };
    return self;
}

- (NSTextField *)labelWithFont:(NSFont *)font color:(NSColor *)color {
    NSTextField *label = [NSTextField labelWithString:@""];
    label.font = font;
    label.textColor = color;
    label.lineBreakMode = NSLineBreakByTruncatingTail;
    label.translatesAutoresizingMaskIntoConstraints = NO;
    return label;
}

- (void)buildInterface {
    NSSplitView *split = [[NSSplitView alloc] init];
    split.vertical = YES;
    split.dividerStyle = NSSplitViewDividerStyleThin;
    split.translatesAutoresizingMaskIntoConstraints = NO;
    self.window.contentView = split;

    NSVisualEffectView *sidebar = [[NSVisualEffectView alloc] init];
    sidebar.material = NSVisualEffectMaterialSidebar;
    sidebar.blendingMode = NSVisualEffectBlendingModeBehindWindow;
    sidebar.state = NSVisualEffectStateFollowsWindowActiveState;
    sidebar.translatesAutoresizingMaskIntoConstraints = NO;

    self.profilePopup = [[NSPopUpButton alloc] init];
    self.profilePopup.target = self;
    self.profilePopup.action = @selector(profileChanged:);
    self.profilePopup.font = [NSFont systemFontOfSize:13 weight:NSFontWeightMedium];
    self.profilePopup.translatesAutoresizingMaskIntoConstraints = NO;
    NSButton *addProfile = [NSButton buttonWithImage:
                                      [NSImage imageWithSystemSymbolName:@"plus"
                                               accessibilityDescription:@"프로필 추가"]
                                          target:self
                                          action:@selector(addProfile:)];
    addProfile.bezelStyle = NSBezelStyleAccessoryBarAction;
    addProfile.toolTip = @"프로필 추가";
    addProfile.translatesAutoresizingMaskIntoConstraints = NO;
    self.removeProfileButton = [NSButton buttonWithImage:
                                          [NSImage imageWithSystemSymbolName:@"minus"
                                                   accessibilityDescription:@"프로필 삭제"]
                                              target:self
                                              action:@selector(removeProfile:)];
    self.removeProfileButton.bezelStyle = NSBezelStyleAccessoryBarAction;
    self.removeProfileButton.toolTip = @"현재 프로필 삭제";
    self.removeProfileButton.translatesAutoresizingMaskIntoConstraints = NO;

    NSScrollView *listScroll = [[NSScrollView alloc] init];
    listScroll.drawsBackground = NO;
    listScroll.hasVerticalScroller = YES;
    listScroll.translatesAutoresizingMaskIntoConstraints = NO;
    self.tableView = [[NSTableView alloc] init];
    self.tableView.headerView = nil;
    self.tableView.rowHeight = 48;
    self.tableView.backgroundColor = NSColor.clearColor;
    self.tableView.style = NSTableViewStyleSourceList;
    self.tableView.delegate = self;
    self.tableView.dataSource = self;
    NSTableColumn *column = [[NSTableColumn alloc] initWithIdentifier:@"package"];
    column.resizingMask = NSTableColumnAutoresizingMask;
    [self.tableView addTableColumn:column];
    listScroll.documentView = self.tableView;
    [sidebar addSubview:self.profilePopup];
    [sidebar addSubview:addProfile];
    [sidebar addSubview:self.removeProfileButton];
    [sidebar addSubview:listScroll];
    [NSLayoutConstraint activateConstraints:@[
        [self.profilePopup.leadingAnchor constraintEqualToAnchor:sidebar.leadingAnchor constant:12],
        [self.profilePopup.topAnchor constraintEqualToAnchor:sidebar.topAnchor constant:12],
        [addProfile.leadingAnchor constraintEqualToAnchor:self.profilePopup.trailingAnchor constant:6],
        [addProfile.centerYAnchor constraintEqualToAnchor:self.profilePopup.centerYAnchor],
        [self.removeProfileButton.leadingAnchor constraintEqualToAnchor:addProfile.trailingAnchor constant:4],
        [self.removeProfileButton.trailingAnchor constraintEqualToAnchor:sidebar.trailingAnchor constant:-10],
        [self.removeProfileButton.centerYAnchor constraintEqualToAnchor:self.profilePopup.centerYAnchor],
        [listScroll.leadingAnchor constraintEqualToAnchor:sidebar.leadingAnchor],
        [listScroll.trailingAnchor constraintEqualToAnchor:sidebar.trailingAnchor],
        [listScroll.topAnchor constraintEqualToAnchor:self.profilePopup.bottomAnchor constant:10],
        [listScroll.bottomAnchor constraintEqualToAnchor:sidebar.bottomAnchor],
        [sidebar.widthAnchor constraintEqualToConstant:280],
    ]];

    NSView *detail = [[NSView alloc] init];
    detail.translatesAutoresizingMaskIntoConstraints = NO;
    self.appIconView = [[NSImageView alloc] init];
    self.appIconView.image = [NSImage imageWithSystemSymbolName:@"apps.iphone"
                                      accessibilityDescription:@"Android applications"];
    self.appIconView.symbolConfiguration =
        [NSImageSymbolConfiguration configurationWithPointSize:38 weight:NSFontWeightRegular];
    self.appIconView.contentTintColor = NSColor.controlAccentColor;
    self.appIconView.imageScaling = NSImageScaleProportionallyUpOrDown;
    self.appIconView.translatesAutoresizingMaskIntoConstraints = NO;
    self.packageLabel = [self labelWithFont:[NSFont systemFontOfSize:24 weight:NSFontWeightSemibold]
                                      color:NSColor.labelColor];
    self.packageLabel.stringValue = @"앱을 선택하세요";
    self.stateLabel = [self labelWithFont:[NSFont systemFontOfSize:13]
                                    color:NSColor.secondaryLabelColor];
    self.stateLabel.stringValue = @"설치된 Android 앱을 관리할 수 있습니다.";

    self.runButton = [NSButton buttonWithTitle:@"실행" target:self action:@selector(runSelected:)];
    self.runButton.bezelStyle = NSBezelStyleRounded;
    self.runButton.keyEquivalent = @"\r";
    self.stopButton = [NSButton buttonWithTitle:@"종료" target:self action:@selector(stopSelected:)];
    self.stopButton.bezelStyle = NSBezelStyleRounded;
    self.dataButton = [NSButton buttonWithTitle:@"앱 데이터 보기"
                                         target:self
                                         action:@selector(revealSelected:)];
    self.dataButton.bezelStyle = NSBezelStyleRounded;
    self.deleteButton = [NSButton buttonWithTitle:@"삭제"
                                           target:self
                                           action:@selector(uninstallSelected:)];
    self.deleteButton.bezelStyle = NSBezelStyleRounded;
    NSStackView *actions = [NSStackView stackViewWithViews:
                                      @[self.runButton, self.stopButton, self.dataButton,
                                        self.deleteButton]];
    actions.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    actions.spacing = 8;
    actions.translatesAutoresizingMaskIntoConstraints = NO;

    NSTextField *logTitle = [self labelWithFont:[NSFont systemFontOfSize:13 weight:NSFontWeightSemibold]
                                         color:NSColor.labelColor];
    logTitle.stringValue = @"런타임 로그";
    NSScrollView *logScroll = [[NSScrollView alloc] init];
    logScroll.borderType = NSBezelBorder;
    logScroll.hasVerticalScroller = YES;
    logScroll.translatesAutoresizingMaskIntoConstraints = NO;
    self.logView = [[NSTextView alloc] init];
    self.logView.editable = NO;
    self.logView.selectable = YES;
    self.logView.font = [NSFont monospacedSystemFontOfSize:11 weight:NSFontWeightRegular];
    self.logView.textColor = NSColor.secondaryLabelColor;
    self.logView.backgroundColor = NSColor.textBackgroundColor;
    self.logView.textContainerInset = NSMakeSize(8, 8);
    logScroll.documentView = self.logView;

    self.daemonLabel = [self labelWithFont:[NSFont systemFontOfSize:11]
                                     color:NSColor.tertiaryLabelColor];
    self.daemonLabel.stringValue = @"daemon 상태 확인 중…";
    self.progress = [[NSProgressIndicator alloc] init];
    self.progress.style = NSProgressIndicatorStyleSpinning;
    self.progress.controlSize = NSControlSizeSmall;
    self.progress.displayedWhenStopped = NO;
    self.progress.translatesAutoresizingMaskIntoConstraints = NO;

    for (NSView *view in @[self.appIconView, self.packageLabel, self.stateLabel, actions, logTitle,
                            logScroll, self.daemonLabel, self.progress]) {
        [detail addSubview:view];
    }
    [NSLayoutConstraint activateConstraints:@[
        [self.appIconView.leadingAnchor constraintEqualToAnchor:detail.leadingAnchor constant:32],
        [self.appIconView.topAnchor constraintEqualToAnchor:detail.topAnchor constant:36],
        [self.appIconView.widthAnchor constraintEqualToConstant:52],
        [self.appIconView.heightAnchor constraintEqualToConstant:52],
        [self.packageLabel.leadingAnchor constraintEqualToAnchor:self.appIconView.trailingAnchor constant:16],
        [self.packageLabel.trailingAnchor constraintLessThanOrEqualToAnchor:detail.trailingAnchor constant:-24],
        [self.packageLabel.topAnchor constraintEqualToAnchor:self.appIconView.topAnchor constant:2],
        [self.stateLabel.leadingAnchor constraintEqualToAnchor:self.packageLabel.leadingAnchor],
        [self.stateLabel.trailingAnchor constraintEqualToAnchor:detail.trailingAnchor constant:-24],
        [self.stateLabel.topAnchor constraintEqualToAnchor:self.packageLabel.bottomAnchor constant:4],
        [actions.leadingAnchor constraintEqualToAnchor:detail.leadingAnchor constant:32],
        [actions.topAnchor constraintEqualToAnchor:self.appIconView.bottomAnchor constant:24],
        [logTitle.leadingAnchor constraintEqualToAnchor:detail.leadingAnchor constant:32],
        [logTitle.topAnchor constraintEqualToAnchor:actions.bottomAnchor constant:28],
        [logScroll.leadingAnchor constraintEqualToAnchor:detail.leadingAnchor constant:32],
        [logScroll.trailingAnchor constraintEqualToAnchor:detail.trailingAnchor constant:-32],
        [logScroll.topAnchor constraintEqualToAnchor:logTitle.bottomAnchor constant:8],
        [logScroll.bottomAnchor constraintEqualToAnchor:self.daemonLabel.topAnchor constant:-14],
        [self.daemonLabel.leadingAnchor constraintEqualToAnchor:detail.leadingAnchor constant:32],
        [self.daemonLabel.bottomAnchor constraintEqualToAnchor:detail.bottomAnchor constant:-18],
        [self.progress.leadingAnchor constraintEqualToAnchor:self.daemonLabel.trailingAnchor constant:8],
        [self.progress.centerYAnchor constraintEqualToAnchor:self.daemonLabel.centerYAnchor],
    ]];

    [split addArrangedSubview:sidebar];
    [split addArrangedSubview:detail];
    [split setPosition:280 ofDividerAtIndex:0];
    [self updateControls];
}

- (void)configureToolbar {
    NSToolbar *toolbar = [[NSToolbar alloc] initWithIdentifier:@"AimToolbar"];
    toolbar.delegate = self;
    toolbar.displayMode = NSToolbarDisplayModeIconOnly;
    self.window.toolbar = toolbar;
}

- (NSArray<NSToolbarItemIdentifier> *)toolbarAllowedItemIdentifiers:(NSToolbar *)toolbar {
    return @[AIMInstallItem, AIMRefreshItem, AIMSystemItem,
             NSToolbarFlexibleSpaceItemIdentifier];
}

- (NSArray<NSToolbarItemIdentifier> *)toolbarDefaultItemIdentifiers:(NSToolbar *)toolbar {
    return @[AIMInstallItem, AIMRefreshItem, NSToolbarFlexibleSpaceItemIdentifier, AIMSystemItem];
}

- (NSToolbarItem *)toolbar:(NSToolbar *)toolbar
       itemForItemIdentifier:(NSToolbarItemIdentifier)identifier
   willBeInsertedIntoToolbar:(BOOL)flag {
    NSToolbarItem *item = [[NSToolbarItem alloc] initWithItemIdentifier:identifier];
    if ([identifier isEqualToString:AIMInstallItem]) {
        item.label = @"APK 설치";
        item.toolTip = @"APK 파일 설치";
        item.image = [NSImage imageWithSystemSymbolName:@"plus.app"
                               accessibilityDescription:item.label];
        item.target = self;
        item.action = @selector(installAPK:);
    } else if ([identifier isEqualToString:AIMRefreshItem]) {
        item.label = @"새로 고침";
        item.toolTip = @"앱과 프로세스 상태 새로 고침";
        item.image = [NSImage imageWithSystemSymbolName:@"arrow.clockwise"
                               accessibilityDescription:item.label];
        item.target = self;
        item.action = @selector(refresh:);
    } else {
        item.label = @"시스템";
        item.toolTip = @"프로필과 런타임 시스템 관리";
        item.image = [NSImage imageWithSystemSymbolName:@"gearshape.2"
                               accessibilityDescription:item.label];
        item.target = self;
        item.action = @selector(manageSystem:);
    }
    return item;
}

- (void)showWindow:(id)sender {
    [super showWindow:sender];
    [self refreshProfilesAndSnapshot];
}

- (void)refreshProfilesAndSnapshot {
    __weak typeof(self) weakSelf = self;
    [self.client fetchProfiles:^(NSArray<NSString *> *profiles, NSError *error) {
        if (error) {
            [weakSelf presentActionError:error];
            return;
        }
        weakSelf.profiles = profiles;
        [weakSelf.profilePopup removeAllItems];
        [weakSelf.profilePopup addItemsWithTitles:profiles];
        [weakSelf.profilePopup selectItemWithTitle:weakSelf.client.profileName];
        weakSelf.removeProfileButton.enabled =
            ![weakSelf.client.profileName isEqualToString:@"default"];
        [weakSelf refresh:nil];
    }];
}

- (NSInteger)numberOfRowsInTableView:(NSTableView *)tableView {
    return self.apps.count;
}

- (NSView *)tableView:(NSTableView *)tableView
    viewForTableColumn:(NSTableColumn *)tableColumn
                   row:(NSInteger)row {
    NSTableCellView *cell = [tableView makeViewWithIdentifier:@"PackageCell" owner:self];
    if (!cell) {
        cell = [[NSTableCellView alloc] init];
        cell.identifier = @"PackageCell";
        NSImageView *image = [[NSImageView alloc] init];
        image.image = [NSImage imageWithSystemSymbolName:@"app"
                                 accessibilityDescription:@"Android app"];
        image.contentTintColor = NSColor.controlAccentColor;
        image.translatesAutoresizingMaskIntoConstraints = NO;
        NSTextField *text = [NSTextField labelWithString:@""];
        text.font = [NSFont systemFontOfSize:13 weight:NSFontWeightMedium];
        text.lineBreakMode = NSLineBreakByTruncatingMiddle;
        text.translatesAutoresizingMaskIntoConstraints = NO;
        NSTextField *detail = [NSTextField labelWithString:@""];
        detail.tag = 100;
        detail.font = [NSFont systemFontOfSize:10];
        detail.textColor = NSColor.secondaryLabelColor;
        detail.lineBreakMode = NSLineBreakByTruncatingMiddle;
        detail.translatesAutoresizingMaskIntoConstraints = NO;
        cell.imageView = image;
        cell.textField = text;
        [cell addSubview:image];
        [cell addSubview:text];
        [cell addSubview:detail];
        [NSLayoutConstraint activateConstraints:@[
            [image.leadingAnchor constraintEqualToAnchor:cell.leadingAnchor constant:8],
            [image.centerYAnchor constraintEqualToAnchor:cell.centerYAnchor],
            [image.widthAnchor constraintEqualToConstant:20],
            [image.heightAnchor constraintEqualToConstant:20],
            [text.leadingAnchor constraintEqualToAnchor:image.trailingAnchor constant:8],
            [text.trailingAnchor constraintEqualToAnchor:cell.trailingAnchor constant:-8],
            [text.topAnchor constraintEqualToAnchor:cell.topAnchor constant:7],
            [detail.leadingAnchor constraintEqualToAnchor:text.leadingAnchor],
            [detail.trailingAnchor constraintEqualToAnchor:text.trailingAnchor],
            [detail.topAnchor constraintEqualToAnchor:text.bottomAnchor constant:1],
        ]];
    }
    AIMInstalledApp *app = self.apps[row];
    cell.textField.stringValue = app.displayName;
    NSTextField *detail = [cell viewWithTag:100];
    detail.stringValue = app.version.length
                             ? [NSString stringWithFormat:@"%@ · %@", app.packageName, app.version]
                             : app.packageName;
    NSImage *icon = app.iconData ? [[NSImage alloc] initWithData:app.iconData] : nil;
    cell.imageView.image = icon ?: [NSImage imageWithSystemSymbolName:@"app"
                                         accessibilityDescription:@"Android app"];
    cell.imageView.contentTintColor = icon ? nil : (self.snapshot.processes[app.packageName].count
                                                         ? NSColor.systemGreenColor
                                                         : NSColor.controlAccentColor);
    return cell;
}

- (void)tableViewSelectionDidChange:(NSNotification *)notification {
    NSInteger row = self.tableView.selectedRow;
    self.selectedPackage = row >= 0 && row < (NSInteger)self.apps.count
                               ? self.apps[row].packageName
                               : nil;
    [self updateControls];
}

- (void)setBusy:(BOOL)busy {
    if (busy) [self.progress startAnimation:nil]; else [self.progress stopAnimation:nil];
}

- (AIMInstalledApp *)selectedApp {
    for (AIMInstalledApp *app in self.apps) {
        if ([app.packageName isEqualToString:self.selectedPackage]) return app;
    }
    return nil;
}

- (void)updateControls {
    NSString *package = self.selectedPackage;
    AIMInstalledApp *app = self.selectedApp;
    NSArray<NSNumber *> *pids = package ? self.snapshot.processes[package] : @[];
    self.packageLabel.stringValue = app.displayName ?: @"앱을 선택하세요";
    NSImage *icon = app.iconData ? [[NSImage alloc] initWithData:app.iconData] : nil;
    self.appIconView.image = icon ?: [NSImage imageWithSystemSymbolName:@"apps.iphone"
                                           accessibilityDescription:@"Android applications"];
    self.appIconView.contentTintColor = icon ? nil : NSColor.controlAccentColor;
    if (!package) {
        self.stateLabel.stringValue = @"설치된 Android 앱을 관리할 수 있습니다.";
    } else if (pids.count) {
        NSString *pidList = [[pids valueForKey:@"stringValue"] componentsJoinedByString:@", "];
        self.stateLabel.stringValue = [NSString stringWithFormat:@"%@ · 실행 중 · PID %@",
                                                                 package, pidList];
    } else {
        self.stateLabel.stringValue = app.version.length
                                          ? [NSString stringWithFormat:@"%@ · 버전 %@ · 설치됨",
                                                                       package, app.version]
                                          : [NSString stringWithFormat:@"%@ · 설치됨", package];
    }
    self.runButton.enabled = package.length > 0 && pids.count == 0;
    self.stopButton.enabled = package.length > 0 && pids.count > 0;
    self.dataButton.enabled = package.length > 0;
    self.deleteButton.enabled = package.length > 0 && pids.count == 0;
}

- (void)appendLog:(NSString *)text {
    if (!text.length) return;
    [self.pendingLogText appendString:text];
    while (YES) {
        NSRange newline = [self.pendingLogText rangeOfString:@"\n"];
        if (newline.location == NSNotFound) break;
        NSString *message = [self.pendingLogText substringToIndex:newline.location];
        [self.pendingLogText deleteCharactersInRange:NSMakeRange(0, NSMaxRange(newline))];
        NSString *line = [NSString stringWithFormat:@"[%@] %@\n",
                           [NSDateFormatter localizedStringFromDate:NSDate.date
                                                           dateStyle:NSDateFormatterNoStyle
                                                           timeStyle:NSDateFormatterMediumStyle],
                           message];
        [self.logView.textStorage appendAttributedString:
             [[NSAttributedString alloc] initWithString:line]];
    }
    if (self.logView.string.length > 200000) {
        NSRange boundary = [self.logView.string rangeOfString:@"\n"
                                                      options:0
                                                        range:NSMakeRange(50000, self.logView.string.length - 50000)];
        NSUInteger length = boundary.location == NSNotFound ? 50000 : NSMaxRange(boundary);
        [self.logView.textStorage deleteCharactersInRange:NSMakeRange(0, length)];
    }
    [self.logView scrollRangeToVisible:NSMakeRange(self.logView.string.length, 0)];
}

- (void)presentActionError:(NSError *)error {
    [self appendLog:[NSString stringWithFormat:@"오류: %@\n", error.localizedDescription]];
    NSAlert *alert = [NSAlert alertWithError:error];
    [alert beginSheetModalForWindow:self.window completionHandler:nil];
}

- (IBAction)refresh:(id)sender {
    [self setBusy:YES];
    __weak typeof(self) weakSelf = self;
    [self.client fetchSnapshot:^(AIMRuntimeSnapshot *snapshot, NSError *error) {
        [weakSelf setBusy:NO];
        if (error) {
            [weakSelf presentActionError:error];
            return;
        }
        weakSelf.snapshot = snapshot;
        weakSelf.apps = snapshot.apps;
        NSString *size = [NSByteCountFormatter stringFromByteCount:(long long)snapshot.allocatedBytes
                                                        countStyle:NSByteCountFormatterCountStyleFile];
        weakSelf.daemonLabel.stringValue =
            [NSString stringWithFormat:@"aimd: %@ · 디스크 %@",
                                       snapshot.daemonStatus, size];
        [weakSelf.tableView reloadData];
        NSInteger row = NSNotFound;
        for (NSUInteger index = 0; index < weakSelf.apps.count; index++) {
            if ([weakSelf.apps[index].packageName isEqualToString:weakSelf.selectedPackage]) {
                row = index;
                break;
            }
        }
        if (row != NSNotFound) {
            [weakSelf.tableView selectRowIndexes:[NSIndexSet indexSetWithIndex:row]
                            byExtendingSelection:NO];
        } else if (weakSelf.apps.count) {
            [weakSelf.tableView selectRowIndexes:[NSIndexSet indexSetWithIndex:0]
                            byExtendingSelection:NO];
        }
        [weakSelf updateControls];
    }];
}

- (IBAction)profileChanged:(id)sender {
    NSString *profile = self.profilePopup.selectedItem.title;
    if (!profile.length || [profile isEqualToString:self.client.profileName]) return;
    self.selectedPackage = nil;
    self.snapshot = nil;
    self.apps = @[];
    [self.client switchToProfile:profile];
    self.window.subtitle = [NSString stringWithFormat:@"%@ 프로필", profile];
    self.removeProfileButton.enabled = ![profile isEqualToString:@"default"];
    [self.tableView reloadData];
    [self updateControls];
    [self refresh:nil];
}

- (IBAction)addProfile:(id)sender {
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"새 Android 프로필";
    alert.informativeText = @"소문자 영문, 숫자, '.', '_' 또는 '-'를 사용하세요.";
    NSTextField *field = [[NSTextField alloc] initWithFrame:NSMakeRect(0, 0, 280, 24)];
    field.placeholderString = @"예: work";
    alert.accessoryView = field;
    [alert addButtonWithTitle:@"생성"];
    [alert addButtonWithTitle:@"취소"];
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse response) {
        if (response != NSAlertFirstButtonReturn || !field.stringValue.length) return;
        NSString *profile = field.stringValue.lowercaseString;
        [self setBusy:YES];
        [self.client createProfile:profile completion:^(NSError *error) {
            [self setBusy:NO];
            if (error) {
                [self presentActionError:error];
                return;
            }
            [self.client switchToProfile:profile];
            self.window.subtitle = [NSString stringWithFormat:@"%@ 프로필", profile];
            self.selectedPackage = nil;
            [self refreshProfilesAndSnapshot];
        }];
    }];
}

- (IBAction)removeProfile:(id)sender {
    NSString *profile = self.client.profileName;
    if ([profile isEqualToString:@"default"]) return;
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = [NSString stringWithFormat:@"%@ 프로필을 삭제하시겠습니까?", profile];
    alert.informativeText = @"설치된 앱, 앱 데이터, 네이티브 캐시와 프로필 디스크가 모두 삭제됩니다.";
    alert.alertStyle = NSAlertStyleCritical;
    [alert addButtonWithTitle:@"프로필 삭제"];
    [alert addButtonWithTitle:@"취소"];
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse response) {
        if (response != NSAlertFirstButtonReturn) return;
        [self setBusy:YES];
        [self.client deleteProfile:profile completion:^(NSError *error) {
            [self setBusy:NO];
            if (error) {
                [self presentActionError:error];
                return;
            }
            [self.client switchToProfile:@"default"];
            self.window.subtitle = @"default 프로필";
            self.selectedPackage = nil;
            [self refreshProfilesAndSnapshot];
        }];
    }];
}

- (IBAction)installAPK:(id)sender {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.title = @"Android APK 설치";
    panel.prompt = @"설치";
    panel.canChooseDirectories = NO;
    panel.allowsMultipleSelection = YES;
    panel.allowedContentTypes = @[[UTType typeWithFilenameExtension:@"apk"]];
    [panel beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse result) {
        if (result != NSModalResponseOK) return;
        [self installAPKURLs:panel.URLs];
    }];
}

- (void)installAPKURLs:(NSArray<NSURL *> *)URLs {
    NSMutableArray<NSURL *> *APKs = [NSMutableArray array];
    for (NSURL *URL in URLs) {
        if (URL.isFileURL && [URL.pathExtension.lowercaseString isEqualToString:@"apk"]) {
            [APKs addObject:URL];
        }
    }
    if (!APKs.count) return;
    [self setBusy:YES];
    [self installAPKs:APKs index:0];
}

- (void)installAPKs:(NSArray<NSURL *> *)APKs index:(NSUInteger)index {
    if (index >= APKs.count) {
        [self setBusy:NO];
        [self refresh:nil];
        return;
    }
    NSURL *URL = APKs[index];
    [self appendLog:[NSString stringWithFormat:@"%@ 설치 중…\n", URL.lastPathComponent]];
    [self.client installAPKAtURL:URL completion:^(NSError *error) {
        if (error) {
            [self setBusy:NO];
            [self presentActionError:error];
            return;
        }
        [self appendLog:[NSString stringWithFormat:@"%@ 설치 완료\n", URL.lastPathComponent]];
        [self installAPKs:APKs index:index + 1];
    }];
}

- (IBAction)runSelected:(id)sender {
    NSString *package = self.selectedPackage;
    if (!package) return;
    [self appendLog:[NSString stringWithFormat:@"%@ 실행 요청\n", package]];
    [self.client launchPackage:package completion:^(NSError *error) {
        if (error) {
            [self presentActionError:error];
            return;
        }
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(1.0 * NSEC_PER_SEC)),
                       dispatch_get_main_queue(), ^{ [self refresh:nil]; });
    }];
}

- (IBAction)stopSelected:(id)sender {
    NSString *package = self.selectedPackage;
    NSArray<NSNumber *> *pids = self.snapshot.processes[package] ?: @[];
    if (!package) return;
    [self.client stopPackage:package pids:pids completion:^(NSError *error) {
        if (error) {
            [self presentActionError:error];
            return;
        }
        [self appendLog:[NSString stringWithFormat:@"%@ 종료 요청\n", package]];
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(0.5 * NSEC_PER_SEC)),
                       dispatch_get_main_queue(), ^{ [self refresh:nil]; });
    }];
}

- (IBAction)revealSelected:(id)sender {
    NSString *package = self.selectedPackage;
    if (!package) return;
    [self.client revealDataForPackage:package completion:^(NSError *error) {
        if (error) [self presentActionError:error];
    }];
}

- (IBAction)uninstallSelected:(id)sender {
    NSString *package = self.selectedPackage;
    if (!package) return;
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = [NSString stringWithFormat:@"%@을 삭제하시겠습니까?", package];
    alert.informativeText = @"APK와 최적화된 코드를 제거합니다. 앱 데이터도 함께 삭제하거나 보관할 수 있습니다.";
    [alert addButtonWithTitle:@"앱과 데이터 삭제"];
    [alert addButtonWithTitle:@"데이터 보관"];
    [alert addButtonWithTitle:@"취소"];
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse response) {
        if (response == NSAlertThirdButtonReturn) return;
        BOOL removeData = response == NSAlertFirstButtonReturn;
        [self setBusy:YES];
        [self.client uninstallPackage:package removeData:removeData completion:^(NSError *error) {
            [self setBusy:NO];
            if (error) {
                [self presentActionError:error];
                return;
            }
            [self appendLog:[NSString stringWithFormat:@"%@ 삭제 완료%@\n", package,
                                                       removeData ? @"" : @" (데이터 보관)"]];
            self.selectedPackage = nil;
            [self refresh:nil];
        }];
    }];
}

- (IBAction)manageSystem:(id)sender {
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"aim 시스템";
    NSArray<NSNumber *> *systemPIDs = self.snapshot.processes[@"android.system"] ?: @[];
    NSString *systemStatus = systemPIDs.count
                                 ? [NSString stringWithFormat:@"실행 중 (PID %@)",
                                                              [[systemPIDs valueForKey:@"stringValue"]
                                                                  componentsJoinedByString:@", "]]
                                 : @"대기 중 · 앱 실행 시 자동 시작";
    NSString *size = [NSByteCountFormatter stringFromByteCount:
                                               (long long)self.snapshot.allocatedBytes
                                                        countStyle:NSByteCountFormatterCountStyleFile];
    alert.informativeText = [NSString stringWithFormat:
        @"프로필: %@\n프로필 디스크: %@\n관리 서비스: %@\nandroid.system: %@\n\n런타임과 호환성 계층은 이 앱 번들에 포함되어 있습니다.",
        self.client.profileName, size, self.snapshot.daemonStatus ?: @"상태 확인 중",
        systemStatus];
    NSButton *loginItem = [NSButton checkboxWithTitle:@"로그인할 때 aim 열기"
                                               target:self
                                               action:@selector(toggleLoginItem:)];
    loginItem.state = SMAppService.mainAppService.status == SMAppServiceStatusEnabled
                          ? NSControlStateValueOn
                          : NSControlStateValueOff;
    loginItem.frame = NSMakeRect(0, 0, 320, 24);
    alert.accessoryView = loginItem;
    [alert addButtonWithTitle:@"프로필 열기"];
    [alert addButtonWithTitle:@"서비스 재시작"];
    [alert addButtonWithTitle:@"취소"];
    [alert beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse response) {
        if (response == NSAlertFirstButtonReturn) {
            [self.client revealProfile:^(NSError *error) {
                if (error) [self presentActionError:error];
            }];
        } else if (response == NSAlertSecondButtonReturn) {
            [self setBusy:YES];
            [self.client restartSystem:^(NSError *error) {
                [self setBusy:NO];
                if (error) {
                    [self presentActionError:error];
                    return;
                }
                [self appendLog:@"aim 시스템 서비스 재시작 완료\n"];
                [self refresh:nil];
            }];
        }
    }];
}

- (IBAction)toggleLoginItem:(NSButton *)sender {
    NSError *error = nil;
    BOOL enabled = sender.state == NSControlStateValueOn;
    BOOL success = enabled
                       ? [SMAppService.mainAppService registerAndReturnError:&error]
                       : [SMAppService.mainAppService unregisterAndReturnError:&error];
    if (!success) {
        sender.state = enabled ? NSControlStateValueOff : NSControlStateValueOn;
        [self presentActionError:error];
    }
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)sender {
    NSPasteboard *pasteboard = sender.draggingPasteboard;
    NSArray<NSURL *> *URLs = [pasteboard readObjectsForClasses:@[NSURL.class]
                                                       options:@{NSPasteboardURLReadingFileURLsOnlyKey: @YES}];
    for (NSURL *URL in URLs) {
        if ([URL.pathExtension.lowercaseString isEqualToString:@"apk"]) {
            return NSDragOperationCopy;
        }
    }
    return NSDragOperationNone;
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)sender {
    NSArray<NSURL *> *URLs = [sender.draggingPasteboard readObjectsForClasses:@[NSURL.class]
                                                                    options:@{NSPasteboardURLReadingFileURLsOnlyKey: @YES}];
    [self installAPKURLs:URLs];
    return URLs.count > 0;
}

@end
