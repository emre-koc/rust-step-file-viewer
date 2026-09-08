/*
 * stepview_ffi.h — C ABI of the `step-ffi` Rust crate (crates/step-ffi).
 *
 * Two independent entry points, both used by the macOS Quick Look extensions:
 *
 *   1. sv_thumbnail_png()  — a STEP file in, PNG bytes out (Finder thumbnails).
 *   2. sv_load() + the sv_mesh_* / sv_instance* accessors — the tessellated model handed over as
 *      plain buffers so a client can build its own scene graph (Space-bar preview).
 *
 * Contract
 * --------
 *   * No function ever unwinds: every entry point catches Rust panics and reports SV_ERR_PANIC
 *     (or a NULL pointer) instead.
 *   * Return codes: 0 = success, 1 = success but degraded, negative = failure.
 *   * sv_last_error() returns a NUL-terminated UTF-8 message describing the last failure *on the
 *     calling thread*. The pointer stays valid until the next step-ffi call on that thread. It is
 *     also set on a degraded (rc == 1) result, to say why.
 *   * Memory handed out by this library is freed only by this library: sv_free_bytes() for PNG
 *     buffers, sv_free() for models. Both accept NULL.
 *   * Units are millimetres throughout; STEP files that declare inches are converted at decode time.
 *   * The on-disk mesh cache written by the StepView app is read but never written here, so a
 *     sandboxed extension cannot race or pollute it.
 *
 * Keep this file in sync with crates/step-ffi/src/lib.rs.
 */

#ifndef STEPVIEW_FFI_H
#define STEPVIEW_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* --- status codes ---------------------------------------------------------------------------- */

/** Full success. */
#define SV_OK 0
/** Success, but degraded: bounding-box silhouette, partial geometry, or a CPU fallback.
 *  sv_last_error() explains which. */
#define SV_DEGRADED 1
/** A NULL or otherwise unusable argument. */
#define SV_ERR_ARGS (-1)
/** The file could not be opened, read, or indexed as STEP. */
#define SV_ERR_OPEN (-2)
/** The file parsed but holds nothing drawable. */
#define SV_ERR_EMPTY (-3)
/** Rendering or PNG encoding failed. */
#define SV_ERR_RENDER (-4)
/** A mesh or instance index was out of range. */
#define SV_ERR_RANGE (-5)
/** A Rust panic was caught at the boundary. */
#define SV_ERR_PANIC (-6)

/* --- diagnostics ------------------------------------------------------------------------------ */

/**
 * Last error on the calling thread, NUL-terminated UTF-8; "" when there is none.
 * Owned by step-ffi; valid until the next step-ffi call on this thread. Never NULL.
 */
const char *sv_last_error(void);

/* --- thumbnails ------------------------------------------------------------------------------- */

/**
 * Render a square isometric thumbnail of @p path and return PNG bytes.
 *
 * The image is @p size_px on a side, RGBA8, with a **fully transparent background** — Finder and
 * Quick Look composite thumbnails over their own backdrop, which differs between light and dark
 * mode, so an opaque plate would look wrong in one of them. The model is drawn shaded with dark
 * feature edges (edges are dropped below 48 px, where they only muddy the image), framed with a
 * ~6% margin, from the standard CAD isometric view.
 *
 * @param path      filesystem path, NUL-terminated UTF-8.
 * @param size_px   edge length in pixels; clamped to 16…4096.
 * @param budget_ms soft wall-clock budget, checked after indexing and after the product structure;
 *                  0 disables it. Over budget, tessellation is skipped and per-shape B-rep bounds
 *                  are drawn as shaded boxes instead — far cheaper, and it still shows the object's
 *                  massing. Tessellation itself also stops at the deadline and whatever finished is
 *                  rendered.
 * @param out_png   receives a malloc'd-by-Rust PNG buffer; free with sv_free_bytes(). Never NULL on
 *                  a non-negative return.
 * @param out_len   receives the buffer length in bytes.
 *
 * @return SV_OK for a full render, SV_DEGRADED when the image is a box silhouette or came from
 *         partial geometry or the CPU fallback, or a negative SV_ERR_* code.
 */
int32_t sv_thumbnail_png(const char *path, uint32_t size_px, uint32_t budget_ms,
                         uint8_t **out_png, size_t *out_len);

/** Free a buffer returned by sv_thumbnail_png(). @p ptr may be NULL. */
void sv_free_bytes(uint8_t *ptr, size_t len);

/* --- model hand-off --------------------------------------------------------------------------- */

/** Opaque handle to a loaded, tessellated STEP file. */
typedef struct sv_model sv_model;

/**
 * Load and tessellate @p path.
 *
 * @param quality 0 = coarse (fast, for previews), 1 = preview (what the app uses, so this hits the
 *                app's mesh cache when it is warm), 2 = fine.
 * @return an owning handle, or NULL on failure — see sv_last_error(). Free with sv_free().
 */
sv_model *sv_load(const char *path, uint32_t quality);

/** Free a model returned by sv_load(). @p m may be NULL. */
void sv_free(sv_model *m);

/**
 * World bounds in millimetres, written as {min_x, min_y, min_z, max_x, max_y, max_z}.
 * Writes six zeros for an empty or NULL model.
 */
void sv_model_bbox(const sv_model *m, double out_min_max[6]);

/** Number of unique (shape, body) meshes. 0 for a NULL model. */
uint32_t sv_mesh_count(const sv_model *m);

/**
 * Buffer sizes for one mesh. Any out-pointer may be NULL.
 *
 * @param vertex_count     vertices; the position and normal streams hold 3×this floats, the colour
 *                         stream 4×this bytes.
 * @param index_count      triangle indices (always a multiple of 3).
 * @param edge_index_count feature-edge indices (a line list, always a multiple of 2; 0 when the
 *                         tessellation quality did not produce edges).
 * @param double_sided     1 for sheet bodies, which must be drawn without back-face culling.
 * @return SV_OK, SV_ERR_ARGS, or SV_ERR_RANGE.
 */
int32_t sv_mesh_info(const sv_model *m, uint32_t mesh, uint32_t *vertex_count,
                     uint32_t *index_count, uint32_t *edge_index_count, uint8_t *double_sided);

/**
 * Copy one mesh's buffers out. Each destination may be NULL to skip that stream; a non-NULL
 * destination must have room for the element count sv_mesh_info() reported.
 *
 * @param positions    3 × vertex_count floats, millimetres, body origin already folded in — so
 *                     (instance matrix) × (position) is the world point.
 * @param normals      3 × vertex_count floats, unit length.
 * @param colors_rgba  4 × vertex_count bytes, straight (non-premultiplied) RGBA taken from the
 *                     B-rep face each vertex belongs to.
 * @param indices      index_count uint32 triangle indices.
 * @param edge_indices edge_index_count uint32 line indices.
 * @return SV_OK, SV_ERR_ARGS, or SV_ERR_RANGE.
 */
int32_t sv_mesh_buffers(const sv_model *m, uint32_t mesh, float *positions, float *normals,
                        uint8_t *colors_rgba, uint32_t *indices, uint32_t *edge_indices);

/** Number of placements: one per (assembly instance × body). 0 for a NULL model. */
uint32_t sv_instance_count(const sv_model *m);

/**
 * One placement.
 *
 * @param mesh                  receives the mesh index to draw. May be NULL.
 * @param matrix_col_major_4x4  receives the world transform, column-major, millimetres — the layout
 *                              simd_float4x4 / SCNMatrix4 already use. May be NULL.
 * @param name_utf8             receives the assembly node name, NUL-terminated and truncated on a
 *                              UTF-8 boundary to fit @p name_cap. May be NULL (pass name_cap 0).
 * @return SV_OK, SV_ERR_ARGS, or SV_ERR_RANGE.
 */
int32_t sv_instance(const sv_model *m, uint32_t i, uint32_t *mesh,
                    float matrix_col_major_4x4[16], char *name_utf8, size_t name_cap);

#ifdef __cplusplus
}
#endif

#endif /* STEPVIEW_FFI_H */
