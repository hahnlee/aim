# Discovery resources

These two unsigned, resource-only archives are synthetic test data, built from
the adjacent XML with the pinned Android 16 aapt2. They contain no app code or
original image resources. `framework.apk` supplies just the attributes these
parsers read; it is not an Android framework implementation or a conformance
oracle. `providers.apk` supplies authenticator, spell-checker and widget XML.

Regenerate from the repository root with:

```sh
python3 crates/aim-services/tests/fixtures/discovery/rebuild.py
```

The XML resource IDs are `0x7f010000` (authenticator), `0x7f010001`
(spellchecker) and `0x7f010002` (widget). The real-session test fixture copies
these immutable archives into its disposable image/data tree. Tests require no
SDK, image or resource compiler at run time and have no missing-input skips.

The enabled-event expectations come from `android-16.0.0_r1`:
`PackageMonitor.doHandlePackageEvent/onPackageChanged`,
`TextServicesManagerService.TextServicesMonitor.onSomePackagesChanged`,
`RegisteredServicesCache.handlePackageEvent`,
`AppWidgetServiceImpl.onPackageBroadcastReceived`,
`MediaRoute2ProviderWatcher`'s package receiver,
`WebViewUpdateService.entirePackageChanged`, and
`PackageManagerService.getSetupWizardPackageNameImpl`.
