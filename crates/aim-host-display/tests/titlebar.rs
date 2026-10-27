//! AppKit must run on the main thread, so this test has its own harness.
//! Checks native properties and rendered chrome without a guest boot.

#[macro_use]
#[allow(dead_code)]
#[path = "../src/bin/aim-display/objc.rs"]
mod objc;
#[path = "../src/bin/aim-display/titlebar.rs"]
mod titlebar;

use objc::{CGRect, CGSize, Id, class, nsstring, release};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

fn components(color: Id) -> [f64; 4] {
    let space = send!(class(c"NSColorSpace"), c"sRGBColorSpace" => Id);
    let color = send!(color, c"colorUsingColorSpace:" => Id, Id = space);
    assert!(!color.is_null());
    [
        send!(color, c"redComponent" => f64),
        send!(color, c"greenComponent" => f64),
        send!(color, c"blueComponent" => f64),
        send!(color, c"alphaComponent" => f64),
    ]
}

fn check(window: Id, expected: [f64; 4], transparent: bool) {
    assert_eq!(
        send!(window, c"titlebarAppearsTransparent" => bool),
        transparent
    );
    let actual = components(send!(window, c"backgroundColor" => Id));
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
    }
}

/// Render AppKit's frame view at its backing scale, including the title
/// bar. This works with the display asleep and never captures the desktop.
fn pixels(window: Id) -> [[f64; 4]; 2] {
    let view = send!(window, c"contentView" => Id);
    let frame = send!(view, c"superview" => Id);
    let bounds = send!(frame, c"bounds" => CGRect);
    send!(frame, c"displayIfNeeded" => ());
    let image = send!(frame, c"bitmapImageRepForCachingDisplayInRect:" => Id, CGRect = bounds);
    assert!(!image.is_null());
    send!(frame, c"cacheDisplayInRect:toBitmapImageRep:" => (), CGRect = bounds, Id = image);
    let scale = send!(image, c"pixelsWide" => isize) as f64 / bounds.size.width;
    [8.0, 100.0].map(|y| {
        components(send!(image, c"colorAtX:y:" => Id,
        isize = (280.0 * scale) as isize, isize = (y * scale) as isize))
    })
}

fn main() {
    let _pool = objc::Pool::new();
    send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let window = send!(class(c"NSWindow"), c"alloc" => Id);
    let window = send!(window, c"initWithContentRect:styleMask:backing:defer:" => Id,
        CGRect = CGRect { size: CGSize { width: 320.0, height: 240.0 }, ..Default::default() },
        usize = 15, usize = 2, bool = false);
    send!(window, c"setReleasedWhenClosed:" => (), bool = false);
    send!(window, c"setTitle:" => (), Id = nsstring("Task colors"));
    let original_frame = send!(window, c"frame" => CGRect);
    let view = send!(window, c"contentView" => Id);
    let content_frame = send!(view, c"frame" => CGRect);
    send!(view, c"setWantsLayer:" => (), bool = true);
    let layer = send!(view, c"layer" => Id);
    let green = send!(class(c"NSColor"), c"greenColor" => Id);
    let green = send!(green, c"CGColor" => Id);
    send!(layer, c"setBackgroundColor:" => (), Id = green);
    let original_pixels = pixels(window);
    let default = components(send!(class(c"NSColor"), c"windowBackgroundColor" => Id));

    titlebar::set(window, 0xff123456, 0xffabcdef);
    check(
        window,
        [0x12, 0x34, 0x56, 255].map(|v| v as f64 / 255.0),
        true,
    );

    // Primary wins, including opaque black; a missing primary takes the
    // background. A later update may clear a previously colored title.
    // Saturated colors avoid dependence on the frame cache's RGB profile.
    for (primary, background, expected) in [
        (0xffff0000, 0xff0000ff, [255, 0, 0, 255]),
        (0xff000000, 0xffffffff, [0, 0, 0, 255]),
        (0, 0xff0000ff, [0, 0, 255, 255]),
    ] {
        titlebar::set(window, primary, background);
        let expected = expected.map(|v| v as f64 / 255.0);
        check(window, expected, true);
        let rendered = pixels(window);
        eprintln!("title {primary:08x}/{background:08x}: {:?}", rendered[0]);
        // The frame cache's RGB profile shifts saturated colors. Check
        // their dominant channel here; the NSColor check stays exact.
        for (actual, expected) in rendered[0].into_iter().zip(expected) {
            assert!(
                if expected == 1.0 {
                    actual > 0.9
                } else {
                    actual < 0.5
                },
                "title pixel {actual} != {expected}"
            );
        }
        assert_eq!(rendered[1], original_pixels[1], "content pixel changed");
    }
    titlebar::set(window, 0, 0);
    check(window, default, false);
    assert_eq!(
        pixels(window),
        original_pixels,
        "default chrome not restored"
    );
    assert_eq!(send!(window, c"styleMask" => usize), 15);
    let current = send!(window, c"frame" => CGRect);
    let content = send!(view, c"frame" => CGRect);
    assert_eq!(current.size.width, original_frame.size.width);
    assert_eq!(current.size.height, original_frame.size.height);
    assert_eq!(content.size.width, content_frame.size.width);
    assert_eq!(content.size.height, content_frame.size.height);
    assert_eq!(objc::text(send!(window, c"title" => Id)), "Task colors");
    send!(window, c"close" => ());
    release(window);
    println!("PASS: AppKit title pixels, fallback, reset and unchanged content/geometry");
}
