//! Assertions/reporting for direct native fixtures, never installed APK policy.
use darwin_art_host::HostOutcome;
use std::{env, error::Error};

pub fn observe(outcome: &HostOutcome) -> Result<(), Box<dyn Error>> {
    println!(
        "ART Darwin DEX interpreter: Hello.answer()={}",
        outcome.process.hello_answer
    );
    println!(
        "ART Darwin JNI: hostPageSize()={} nativeRoundTrip()={}",
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) },
        outcome.process.native_round_trip
    );
    println!(
        "ART runtime native: System.arraycopy()={}",
        outcome.process.arraycopy_result
    );
    if let (Ok(package), Ok(activity)) = (
        env::var("DARWIN_ART_APK_APP_PACKAGE"),
        env::var("DARWIN_ART_APK_APP_ACTIVITY"),
    ) {
        let scale = match env::var("DARWIN_ART_WINDOW_SCALE").as_deref() {
            Ok("2") => 2,
            Ok("1") | Err(_) => 1,
            _ => return Err("DARWIN_ART_WINDOW_SCALE must be 1 or 2".into()),
        };
        let geometry = |w: u32, h: u32| {
            (w == 360 * scale && h == 640 * scale) || (w == 640 * scale && h == 360 * scale)
        };
        let native =
            u8::from(env::var("DARWIN_ART_APK_APP_NATIVE_PATH").is_ok_and(|p| !p.is_empty()));
        let widget = if env::var("DARWIN_ART_APK_APP_EXPECT_WIDGETS").as_deref() == Ok("1") {
            " widgets=framework-owned"
        } else {
            ""
        };
        if let Some(frame) = outcome.last_frame.as_ref() {
            if !geometry(frame.width, frame.height)
                || outcome.frames_presented == 0
                || !frame
                    .argb_pixels
                    .iter()
                    .all(|p| p & 0xff00_0000 == 0xff00_0000)
            {
                return Err("APK Activity frame did not match its opaque frame contract".into());
            }
            let pixels = u64::from(frame.width) * u64::from(frame.height);
            println!(
                "ART Android APK: package={package} launcher={activity} classes.dex=APK native={native} pixels={pixels}/opaque{widget}"
            );
        } else {
            if outcome.frames_presented == 0
                || !geometry(outcome.process.frame_width, outcome.process.frame_height)
            {
                return Err(
                    "APK Activity GPU presentation did not match its frame contract".into(),
                );
            }
            println!(
                "ART Android APK: package={package} launcher={activity} classes.dex=APK native={native} gpu=direct drawable={}x{}{widget}",
                outcome.process.frame_width, outcome.process.frame_height
            );
        }
    } else {
        println!(
            "ART Android framework: ProbeActivity().probeValue()={}",
            outcome.process.activity_probe_result
        );
    }
    println!("ART Android window: Activity.attach()=PhoneWindow+DecorView");
    println!(
        "ART Android view: Activity.setContentView()->DecorView.draw(Canvas)={}x{}",
        outcome.process.frame_width, outcome.process.frame_height
    );
    println!(
        "ART Android lifecycle: Activity.onCreate()={}",
        outcome.process.lifecycle_result
    );
    println!("ART Darwin launcher: main(String[])=ok");
    Ok(())
}
