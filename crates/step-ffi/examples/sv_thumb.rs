//! Dev harness for the FFI thumbnail path — the same call the Quick Look extension makes, without
//! Quick Look in the way.
//!
//!     cargo run --release -p step-ffi --example sv_thumb -- <file.step> [size] [budget_ms] [out.png]

use std::ffi::{CStr, CString};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: sv_thumb <file.step> [size] [budget_ms] [out.png]");
        std::process::exit(2);
    };
    let size: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(512);
    let budget: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(900);
    let out = args.next().unwrap_or_else(|| "build/ql/sv_thumb.png".into());

    let c = CString::new(path.clone()).unwrap();
    let (mut png, mut len) = (std::ptr::null_mut(), 0usize);
    let t = std::time::Instant::now();
    let rc = step_ffi::sv_thumbnail_png(c.as_ptr(), size, budget, &mut png, &mut len);
    let ms = t.elapsed().as_secs_f32() * 1000.0;
    // SAFETY: `sv_last_error` always returns a valid NUL-terminated pointer.
    let err = unsafe { CStr::from_ptr(step_ffi::sv_last_error()) }.to_string_lossy().into_owned();
    println!("rc {rc} in {ms:.0} ms, {len} bytes{}", if err.is_empty() { String::new() } else { format!(" ({err})") });
    if rc < 0 {
        std::process::exit(1);
    }
    // SAFETY: a non-negative return means `png`/`len` describe a live buffer.
    let bytes = unsafe { std::slice::from_raw_parts(png, len) };
    if let Some(p) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(p).ok();
    }
    std::fs::write(&out, bytes).unwrap();
    let img = image::load_from_memory(bytes).unwrap().to_rgba8();
    let opaque = img.pixels().filter(|p| p.0[3] > 128).count();
    println!("wrote {out}: {}x{}, {opaque} opaque px ({:.0}% coverage)", img.width(), img.height(), 100.0 * opaque as f32 / (img.width() * img.height()) as f32);
    step_ffi::sv_free_bytes(png, len);
}
