//! Exact AOSP 16 provider imports demonstrated by secondary native clients.
//! This exports real implementations, never test JNI or assertion wrappers.
use std::process::Command;

const CORE: &[&str] = &[
    // These AppKit input/title/wake operations are shared by both flavors.
    "_darwin_art_surface_set_input_sink",
    "_darwin_art_android_input_sink_install",
    "_darwin_art_surface_set_active_title",
    "_darwin_art_surface_set_owner_wake",
    // Real receiver, transport and balanced provider operations consumed by
    // separate clients. Synthetic roots/service scaffolding stay test-owned.
    "__ZN10darwin_art24ReceiveServiceBindIntentEP7_JNIEnvi",
    "__ZN10darwin_art24StartServingRemoteBinderEP7_JNIEnviP8_jobject",
    "__ZN10darwin_art30DispatchFrameworkPendingVsyncsEP7_JNIEnvx",
    "__ZN10darwin_art27DarwinAngleHostSurfaceWidthEv",
    "__ZN10darwin_art28DarwinAngleHostSurfaceHeightEv",
    "__ZN10darwin_art33ConfigureDarwinAngleDisplayTargetEii",
    "__ZN10darwin_art9providers15acquire_networkEPNSt3__112basic_stringIcNS1_11char_traitsIcEENS1_9allocatorIcEEEE",
    "__ZN10darwin_art9providers15release_networkEv",
    "_darwin_art_android_ANativeWindow_create",
    "_darwin_art_android_ANativeWindow_getFormat",
    "_darwin_art_android_ANativeWindow_getHeight",
    "_darwin_art_android_ANativeWindow_getWidth",
    "_darwin_art_android_ANativeWindow_is_managed",
    "_darwin_art_android_ANativeWindow_release",
    "_darwin_art_android_ANativeWindow_setBuffersGeometry",
    "_darwin_art_android_platform_poll_current_looper",
    "_darwin_art_android_platform_prepare_current_looper",
    "_darwin_art_android_platform_wake_looper",
    "_darwin_art_android_surface_control_create_root",
    "__ZN3art10ThreadList18RunEmptyCheckpointEv",
    "__ZN3art11interpreter16IsNterpSupportedEv",
    "__ZN3art11interpreter16UnstartedRuntime10InitializeEv",
    "__ZN3art11interpreter18CanRuntimeUseNterpEv",
    "__ZN3art11interpreter18GetNterpEntryPointEv",
    "__ZN3art11interpreter26EnterInterpreterFromInvokeEPNS_6ThreadEPNS_9ArtMethodENS_6ObjPtrINS_6mirror6ObjectEEEPjPNS_6JValueEb",
    "__ZN3art13ParsedOptions5ParseERKNSt3__16vectorINS1_4pairINS1_12basic_stringIcNS1_11char_traitsIcEENS1_9allocatorIcEEEEPKvEENS7_ISC_EEEEbPNS_18RuntimeArgumentMapE",
    "__ZN3art13ProfilingInfo14GetInlineCacheEj",
    "__ZN3art15instrumentation23InstrumentationListener12FieldWrittenEPNS_6ThreadENS_6HandleINS_6mirror6ObjectEEEPNS_9ArtMethodEjPNS_8ArtFieldES7_",
    "__ZN3art16InterpreterCache5ClearEPNS_6ThreadE",
    "__ZN3art22ScopedProfilingInfoUseC1EPNS_3jit3JitEPNS_9ArtMethodEPNS_6ThreadE",
    "__ZN3art22ScopedProfilingInfoUseD1Ev",
    "__ZN3art22ThrowNoSuchMethodErrorENS_6ObjPtrINS_6mirror5ClassEEENSt3__117basic_string_viewIcNS4_11char_traitsIcEEEERKNS_9SignatureE",
    "__ZN3art23ComputeModifiedUtf8HashEPKc",
    "__ZN3art2ti9AgentSpecC1ERKNSt3__112basic_stringIcNS2_11char_traitsIcEENS2_9allocatorIcEEEE",
    "__ZN3art3jit12JitCodeCache13IsOsrCompiledEPNS_9ArtMethodE",
    "__ZN3art3jit3Jit13PrepareForOsrEPNS_9ArtMethodEjPj",
    "__ZN3art6Plugin6UnloadEv",
    "__ZN3art6PluginC1ERKS0_",
    "__ZN3art6Thread31MadviseAwayAlternateSignalStackEv",
    "__ZN3art6mirror9Throwable8GetCauseEv",
    "__ZN3art7Runtime19DetachCurrentThreadEb",
    "__ZNK3art11Instruction10DumpStringEPKNS_7DexFileE",
    "__ZNK3art11Instruction15GetTargetOffsetEv",
    "__ZNK3art11Instruction28SizeInCodeUnitsComplexOpcodeEv",
    "__ZNK3art2gc4Heap10GetGcCountEv",
    "__ZTIN3art15instrumentation23InstrumentationListenerE",
    "_art_quick_osr_stub",
    "_art_quick_to_interpreter_bridge",
    "__ZN11unwindstack15AndroidUnwinder6UnwindERNS_19AndroidUnwinderDataE",
    "__ZNK11unwindstack15AndroidUnwinder11FormatFrameERKNS_9FrameDataE",
    "__ZTVN11unwindstack20AndroidLocalUnwinderE",
    "__ZTVN11unwindstack21AndroidRemoteUnwinderE",
    "__ZN7android4base10LogMessage6streamEv",
    "__ZN7android4base10LogMessageC1EPKcjNS0_11LogSeverityES3_i",
    "__ZN7android4base10LogMessageD1Ev",
    "__ZN7android4base21SetMinimumLogSeverityENS0_11LogSeverityE",
    "__ZN7android4base9ShouldLogENS0_11LogSeverityEPKc",
];

const GRAPHICS: &[&str] = &[
    "__ZN7SkPaint12setBlendModeE11SkBlendMode",
    "__ZN7SkPaint8setColorEj",
    "__ZN7SkPaintC1Ev",
    "__ZN7SkPaintD1Ev",
    "__ZN7android10uirenderer10RenderNode14syncPropertiesEv",
    "__ZN7android10uirenderer10RenderNode15syncDisplayListERNS0_12TreeObserverEPNS0_8TreeInfoE",
    "__ZN7android10uirenderer10RenderNode17onRemovedFromTreeEPNS0_8TreeInfoE",
    "__ZN7android10uirenderer12renderthread8TimeLord13vsyncReceivedExxxxx",
    "__ZN7android10uirenderer12renderthread8TimeLordC1Ev",
    "__ZN7android10uirenderer12skiapipeline15SkiaDisplayList14updateChildrenENSt3__18functionIFvPNS0_10RenderNodeEEEE",
    "__ZN7android10uirenderer12skiapipeline18RenderNodeDrawableC1EPNS0_10RenderNodeEP8SkCanvasbb",
    "__ZN7android10uirenderer12skiapipeline18RenderNodeDrawableD1Ev",
    "__ZN7android10uirenderer15AnimationHandle19notifyAnimationsRanEv",
    "__ZN7android10uirenderer15AnimatorManager11pushStagingEv",
    "__ZN7android10uirenderer16AnimationContext22addAnimatingRenderNodeERNS0_10RenderNodeE",
    "__ZN7android10uirenderer16AnimationContextC1ERNS0_12renderthread8TimeLordE",
    "__ZN7android10uirenderer16RenderProperties12updateMatrixEv",
    "__ZN7android10uirenderer22BaseRenderNodeAnimator7animateERNS0_16AnimationContextE",
    "__ZN7android14sp_report_raceEv",
    "__ZN8SkCanvas10drawCircleEfffRK7SkPaint",
    "__ZN8SkCanvas4saveEv",
    "__ZN8SkCanvas7restoreEv",
    "__ZN8SkCanvas8clipRectERK6SkRect8SkClipOpb",
    "__ZN8SkCanvas8drawRectERK6SkRectRK7SkPaint",
    "__ZN8SkCanvas9drawColorERK8SkRGBA4fIL11SkAlphaType3EE11SkBlendMode",
    "__ZN8SkCanvas9saveLayerEPK6SkRectPK7SkPaint",
    "__ZN8SkCanvas9translateEff",
    "__ZN8SkRGBA4fIL11SkAlphaType3EE9FromColorEj",
    "__ZNK7android10uirenderer12skiapipeline18RenderNodeDrawable9forceDrawEP8SkCanvas",
    "_darwin_art_surface_gpu_begin",
    "_darwin_art_surface_gpu_canvas",
    "_darwin_art_surface_gpu_composite_embedded",
    "_darwin_art_surface_gpu_end",
    "_darwin_art_surface_gpu_iosurface_id",
    "_darwin_art_surface_set_active_gpu",
    "_darwin_art_surface_set_title",
];

pub(super) fn apply(command: &mut Command, graphics: bool) {
    for symbol in CORE
        .iter()
        .chain(if graphics { GRAPHICS.iter() } else { [].iter() })
    {
        command.arg(format!("-Wl,-exported_symbol,{symbol}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_exports_are_exact_and_have_no_test_implementation() {
        let mut seen = std::collections::BTreeSet::new();
        for symbol in CORE.iter().chain(GRAPHICS) {
            assert!(seen.insert(symbol));
            assert!(!symbol.contains('*') && !symbol.contains("Java_Main"));
            assert!(!symbol.contains("fixture") && !symbol.contains("unwindstack_check"));
            assert!(!symbol.contains("VerifyDarwinMediaCodecSurfaceLifecycle"));
            assert!(
                !symbol.contains("debug_entrypoint")
                    && !symbol.contains("FocusFrameworkViewRoot")
                    && !symbol.contains("SetFrameworkViewRootFocus")
                    && !symbol.contains("DispatchFrameworkInputEventEP")
                    && !symbol.contains("DispatchFrameworkInputEventResultEP")
                    && !symbol.contains("DispatchInputPacketEP")
                    && !symbol.contains("AcquireInputReceiver")
                    && !symbol.contains("RegisterReceiverFinish")
                    && !symbol.contains("TakeReceiverFinish")
                    && !symbol.contains("CancelReceiverFinish")
                    && !symbol.contains("CloseReceiverFinishObservation")
                    && !symbol.contains("ReceiverCallbackAdmission")
            );
        }
    }
}
