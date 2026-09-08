//! `step-ffi`: the C ABI stepview's macOS Quick Look extensions link against.
//!
//! Two independent entry points:
//!
//! * [`sv_thumbnail_png`] — one call in, PNG bytes out. Used by the thumbnail extension.
//! * [`sv_load`] and the `sv_mesh_*` / `sv_instance*` accessors — hand the tessellated model over as
//!   plain buffers so the preview extension can build `SCNGeometry` objects and share one geometry
//!   between every instance of a part. Used by the preview extension.
//!
//! # Contract
//!
//! * Every exported function catches panics ([`std::panic::catch_unwind`]); nothing unwinds across
//!   the boundary. A caught panic is reported as [`SV_ERR_PANIC`] (or a null pointer).
//! * Negative return values are errors, `0` is success and `1` means "succeeded, but degraded".
//! * [`sv_last_error`] returns a NUL-terminated, thread-local message describing the last failure on
//!   the calling thread. The pointer stays valid until the next `step-ffi` call on that thread.
//! * Pointers handed out by this library are freed only by this library: [`sv_free_bytes`] for PNG
//!   bytes, [`sv_free`] for models.
//! * The mesh cache is read-only here — a Quick Look extension never writes it.
//!
//! The hand-written header is `macos/QuickLook/include/stepview_ffi.h`; keep the two in step.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

pub mod load;
pub mod model;
pub mod raster;
pub mod thumb;

#[cfg(test)]
mod tests;

// --- status codes -------------------------------------------------------------------------------

/// Full success.
pub const SV_OK: i32 = 0;
/// Succeeded, but the result is degraded (bounding boxes, partial geometry, or a CPU fallback).
pub const SV_DEGRADED: i32 = 1;
/// A null or otherwise unusable argument.
pub const SV_ERR_ARGS: i32 = -1;
/// The file could not be opened, read or indexed as STEP.
pub const SV_ERR_OPEN: i32 = -2;
/// The file parsed but contains nothing drawable.
pub const SV_ERR_EMPTY: i32 = -3;
/// Rendering or PNG encoding failed.
pub const SV_ERR_RENDER: i32 = -4;
/// A mesh or instance index was out of range.
pub const SV_ERR_RANGE: i32 = -5;
/// A panic was caught at the FFI boundary.
pub const SV_ERR_PANIC: i32 = -6;

// --- thread-local error message -----------------------------------------------------------------

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

pub(crate) fn set_error(msg: impl Into<String>) {
    let msg = msg.into();
    let c = CString::new(msg.replace('\0', " ")).unwrap_or_else(|_| CString::default());
    let _ = LAST_ERROR.try_with(|e| *e.borrow_mut() = c);
}

fn clear_error() {
    let _ = LAST_ERROR.try_with(|e| *e.borrow_mut() = CString::default());
}

/// The last error on the calling thread as a NUL-terminated UTF-8 string (empty if none).
///
/// The pointer is owned by `step-ffi` and stays valid until the next `step-ffi` call on this thread.
#[unsafe(no_mangle)]
pub extern "C" fn sv_last_error() -> *const c_char {
    // The `CString` lives in thread-local storage, so the pointer outlives the borrow guard.
    LAST_ERROR.try_with(|e| e.borrow().as_ptr()).unwrap_or(c"".as_ptr())
}

// --- helpers ------------------------------------------------------------------------------------

/// Run `f`, turning a panic into `on_panic` and a recorded error message.
fn guard<T>(on_panic: T, f: impl FnOnce() -> T) -> T {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            set_error("step-ffi caught a panic");
            on_panic
        }
    }
}

/// Borrow a C string as a filesystem path.
fn path_arg(p: *const c_char) -> Option<PathBuf> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller promises a NUL-terminated string; checked non-null above.
    let s = unsafe { CStr::from_ptr(p) };
    s.to_str().ok().map(PathBuf::from)
}

/// Borrow an opaque model handle.
fn model_arg<'a>(m: *const model::Model) -> Option<&'a model::Model> {
    if m.is_null() {
        return None;
    }
    // SAFETY: `m` came from `sv_load` and has not been freed (caller contract).
    Some(unsafe { &*m })
}

/// Hand a byte buffer to C. Freed by [`sv_free_bytes`].
fn leak_bytes(v: Vec<u8>) -> (*mut u8, usize) {
    let boxed = v.into_boxed_slice();
    let len = boxed.len();
    (Box::into_raw(boxed) as *mut u8, len)
}

/// Copy `src` into `dst` when `dst` is non-null.
///
/// # Safety
/// `dst` must be null or point to at least `src.len()` writable elements.
unsafe fn write_slice<T: Copy>(dst: *mut T, src: &[T]) {
    if dst.is_null() || src.is_empty() {
        return;
    }
    unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len()) };
}

// --- thumbnail ----------------------------------------------------------------------------------

/// Render a square isometric thumbnail of `path` and return PNG bytes.
///
/// * `size_px` — edge length in pixels (clamped to 16…4096).
/// * `budget_ms` — soft wall-clock budget; `0` means no budget.
/// * `out_png` / `out_len` — receive the PNG buffer; free it with [`sv_free_bytes`].
///
/// Returns [`SV_OK`] for a full render, [`SV_DEGRADED`] when the image is a bounding-box silhouette
/// or was rendered from partial geometry, and a negative code on failure.
#[unsafe(no_mangle)]
pub extern "C" fn sv_thumbnail_png(
    path: *const c_char,
    size_px: u32,
    budget_ms: u32,
    out_png: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    guard(SV_ERR_PANIC, || {
        clear_error();
        if out_png.is_null() || out_len.is_null() {
            set_error("sv_thumbnail_png: null output pointer");
            return SV_ERR_ARGS;
        }
        // SAFETY: both pointers checked non-null.
        unsafe {
            *out_png = std::ptr::null_mut();
            *out_len = 0;
        }
        let Some(path) = path_arg(path) else {
            set_error("sv_thumbnail_png: path is null or not UTF-8");
            return SV_ERR_ARGS;
        };
        match thumb::thumbnail(&path, size_px, budget_ms) {
            Ok(t) => {
                let (ptr, len) = leak_bytes(t.png);
                // SAFETY: both pointers checked non-null above.
                unsafe {
                    *out_png = ptr;
                    *out_len = len;
                }
                t.code
            }
            Err((code, msg)) => {
                set_error(msg);
                code
            }
        }
    })
}

/// Free a buffer returned by [`sv_thumbnail_png`]. `ptr` may be null.
#[unsafe(no_mangle)]
pub extern "C" fn sv_free_bytes(ptr: *mut u8, len: usize) {
    guard((), || {
        if ptr.is_null() || len == 0 {
            return;
        }
        // SAFETY: reconstructs exactly the `Box<[u8]>` that `leak_bytes` produced.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) });
    })
}

// --- model --------------------------------------------------------------------------------------

/// Load and tessellate `path`. `quality`: 0 = coarse, 1 = preview, 2 = fine.
///
/// Returns an opaque handle, or null on failure (see [`sv_last_error`]). Free with [`sv_free`].
#[unsafe(no_mangle)]
pub extern "C" fn sv_load(path: *const c_char, quality: u32) -> *mut model::Model {
    guard(std::ptr::null_mut(), || {
        clear_error();
        let Some(path) = path_arg(path) else {
            set_error("sv_load: path is null or not UTF-8");
            return std::ptr::null_mut();
        };
        let params = model::params_for(quality);
        let loaded = match load::load(&path, &params, None) {
            Ok(l) => l,
            Err(e) => {
                set_error(e);
                return std::ptr::null_mut();
            }
        };
        let m = model::Model::build(&loaded.structure, &loaded.meshes);
        if m.meshes.is_empty() {
            set_error(format!("{}: no drawable geometry", path.display()));
            return std::ptr::null_mut();
        }
        Box::into_raw(Box::new(m))
    })
}

/// Free a model returned by [`sv_load`]. `m` may be null.
#[unsafe(no_mangle)]
pub extern "C" fn sv_free(m: *mut model::Model) {
    guard((), || {
        if m.is_null() {
            return;
        }
        // SAFETY: `m` came from `sv_load` and is freed exactly once (caller contract).
        drop(unsafe { Box::from_raw(m) });
    })
}

/// World-space bounds in millimetres: `[min_x, min_y, min_z, max_x, max_y, max_z]`.
/// An empty model writes six zeros.
#[unsafe(no_mangle)]
pub extern "C" fn sv_model_bbox(m: *const model::Model, out_min_max: *mut f64) {
    guard((), || {
        if out_min_max.is_null() {
            return;
        }
        let bb = model_arg(m).map(|m| m.bbox).filter(|b| !b.is_empty());
        let v: [f64; 6] = match bb {
            Some(b) => [b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z],
            None => [0.0; 6],
        };
        // SAFETY: caller provides at least 6 writable doubles; checked non-null.
        unsafe { write_slice(out_min_max, &v) };
    })
}

/// Number of unique `(shape, body)` meshes.
#[unsafe(no_mangle)]
pub extern "C" fn sv_mesh_count(m: *const model::Model) -> u32 {
    guard(0, || model_arg(m).map_or(0, |m| m.meshes.len() as u32))
}

/// Buffer sizes for one mesh. Any out-pointer may be null.
///
/// `vertex_count` is the element count of the position / normal / colour streams (which hold
/// `3·vc`, `3·vc` and `4·vc` components respectively); `index_count` and `edge_index_count` are
/// element counts of the triangle and line index streams.
#[unsafe(no_mangle)]
pub extern "C" fn sv_mesh_info(
    m: *const model::Model,
    mesh: u32,
    vertex_count: *mut u32,
    index_count: *mut u32,
    edge_index_count: *mut u32,
    double_sided: *mut u8,
) -> i32 {
    guard(SV_ERR_PANIC, || {
        clear_error();
        let Some(model) = model_arg(m) else {
            set_error("sv_mesh_info: null model");
            return SV_ERR_ARGS;
        };
        let Some(mesh) = model.meshes.get(mesh as usize) else {
            set_error("sv_mesh_info: mesh index out of range");
            return SV_ERR_RANGE;
        };
        // SAFETY: each pointer is null-checked by `write_slice`.
        unsafe {
            write_slice(vertex_count, &[mesh.vertex_count]);
            write_slice(index_count, &[mesh.indices.len() as u32]);
            write_slice(edge_index_count, &[mesh.edge_indices.len() as u32]);
            write_slice(double_sided, &[mesh.double_sided as u8]);
        }
        SV_OK
    })
}

/// Copy one mesh's buffers out. Every destination may be null to skip that stream; a non-null
/// destination must hold the element count [`sv_mesh_info`] reported for it.
///
/// Positions are millimetres with the body origin already folded in, so multiplying by an instance
/// matrix from [`sv_instance`] gives world millimetres. Colours are RGBA8, one per vertex, taken
/// from the B-rep face the vertex belongs to.
#[unsafe(no_mangle)]
pub extern "C" fn sv_mesh_buffers(
    m: *const model::Model,
    mesh: u32,
    positions: *mut f32,
    normals: *mut f32,
    colors_rgba: *mut u8,
    indices: *mut u32,
    edge_indices: *mut u32,
) -> i32 {
    guard(SV_ERR_PANIC, || {
        clear_error();
        let Some(model) = model_arg(m) else {
            set_error("sv_mesh_buffers: null model");
            return SV_ERR_ARGS;
        };
        let Some(mesh) = model.meshes.get(mesh as usize) else {
            set_error("sv_mesh_buffers: mesh index out of range");
            return SV_ERR_RANGE;
        };
        // SAFETY: each pointer is null-checked by `write_slice`; sizes come from `sv_mesh_info`.
        unsafe {
            write_slice(positions, &mesh.positions);
            write_slice(normals, &mesh.normals);
            write_slice(colors_rgba, &mesh.colors);
            write_slice(indices, &mesh.indices);
            write_slice(edge_indices, &mesh.edge_indices);
        }
        SV_OK
    })
}

/// Number of placements: one per `(assembly instance, body)`.
#[unsafe(no_mangle)]
pub extern "C" fn sv_instance_count(m: *const model::Model) -> u32 {
    guard(0, || model_arg(m).map_or(0, |m| m.instances.len() as u32))
}

/// One placement: which mesh to draw, its column-major 4×4 world matrix in millimetres, and the
/// assembly node name.
///
/// `mesh`, `matrix_col_major_4x4` and `name_utf8` may each be null. When `name_utf8` is non-null and
/// `name_cap` is at least 1 the name is written NUL-terminated, truncated to fit.
#[unsafe(no_mangle)]
pub extern "C" fn sv_instance(
    m: *const model::Model,
    i: u32,
    mesh: *mut u32,
    matrix_col_major_4x4: *mut f32,
    name_utf8: *mut c_char,
    name_cap: usize,
) -> i32 {
    guard(SV_ERR_PANIC, || {
        clear_error();
        let Some(model) = model_arg(m) else {
            set_error("sv_instance: null model");
            return SV_ERR_ARGS;
        };
        let Some(inst) = model.instances.get(i as usize) else {
            set_error("sv_instance: instance index out of range");
            return SV_ERR_RANGE;
        };
        // SAFETY: each pointer is null-checked by `write_slice`.
        unsafe {
            write_slice(mesh, &[inst.mesh]);
            write_slice(matrix_col_major_4x4, &inst.matrix);
        }
        if !name_utf8.is_null() && name_cap > 0 {
            let bytes = inst.name.as_bytes();
            // Truncate on a UTF-8 boundary so the result is still a valid string.
            let mut n = bytes.len().min(name_cap - 1);
            while n > 0 && n < bytes.len() && (bytes[n] & 0xC0) == 0x80 {
                n -= 1;
            }
            // SAFETY: `n < name_cap` and the caller provides `name_cap` writable bytes.
            unsafe {
                write_slice(name_utf8 as *mut u8, &bytes[..n]);
                *name_utf8.add(n) = 0;
            }
        }
        SV_OK
    })
}
