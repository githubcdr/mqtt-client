//! macOS desktop integration: sets the Dock application icon at runtime so
//! even bare binaries launched via `cargo run` display the app icon.

use crate::icon::{encode_png_rgba, render_icon_rgba};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

/// Render the procedural icon and apply it to `NSApplication.applicationIconImage`.
///
/// Must be called on the main thread after `NSApplication` has been initialized
/// (e.g., inside `eframe::run_native`'s setup callback).
pub fn set_dock_icon() {
    let size = 256usize;
    let rgba = render_icon_rgba(size);
    let png_bytes = encode_png_rgba(size as u32, size as u32, &rgba);

    // SAFETY: We are on the main AppKit thread after eframe/winit initialized
    // NSApplication, and we check every Objective-C pointer for null before use.
    unsafe {
        let ns_data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: png_bytes.as_ptr() as *const std::ffi::c_void,
            length: png_bytes.len()
        ];
        if ns_data.is_null() {
            return;
        }

        let ns_image_alloc: *mut AnyObject = msg_send![class!(NSImage), alloc];
        if ns_image_alloc.is_null() {
            return;
        }
        let ns_image: *mut AnyObject = msg_send![ns_image_alloc, initWithData: ns_data];
        if ns_image.is_null() {
            return;
        }

        let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        if !ns_app.is_null() {
            let _: () = msg_send![ns_app, setApplicationIconImage: ns_image];
        }

        let _: () = msg_send![ns_image, release];
    }
}
