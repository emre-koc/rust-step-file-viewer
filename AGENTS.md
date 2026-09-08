# stepview agent notes

- Rust 1.98 stable via Homebrew (no rustup, no nightly). `cargo build --release`,
  `cargo test --workspace --release`, `cargo clippy --workspace`, `macos/build.sh [--install]`.
- Crate graph (dependencies point downward only):
  `step-p21` ← `step-model` ← `step-brep` ← {`step-cache`, `step-export`, `step-render`} ← {`stepview`, `step-ffi`};
  `step-mesh` is the geometry kernel (types + tessellator) and is depended on by `step-model`/`step-brep` for
  the `Surface`/`Curve3`/`ShapeTopology` types. Do not add upward dependencies.
- Small generated fixtures live in `tests/fixtures/` (`tools/gen_fixtures.py`). Larger real-world STEP
  files used for benchmarks are not part of this repo: `tests/root_fixtures.rs` looks for them under
  `STEPVIEW_FIXTURES_DIR` (default: the parent directory) and skips with a message when they are absent.
- Write only to `target*/`, `build/`, and `~/Library/Caches/stepview/`.
- Performance budgets on the 74 MB Creo benchmark assembly (M1 Max, release): index ≤ 150 ms (measured ~20),
  product structure ≤ 250 ms (~50), full tessellation ≤ 1.5 s (~0.4 s), cache hit ≤ 100 ms (~0).
  `stepview bench <file>` reports each stage; `tests/root_fixtures.rs` asserts the budgets in release.
- Quality gates: `stepview info --mesh <file>` must report 0 skipped faces on the benchmark assembly and the generated fixtures
  (asserted in tests). A face that fails to tessellate is a diagnostic, never an abort. Unknown entity kinds
  are `Unsupported`, not errors.
- All geometry is converted to millimetres at decode time, per representation context (files mix inch and mm).
  Assembly child/parent representations are decided by ownership, never by argument position (Creo and
  SolidWorks order them oppositely).
- Version pins that matter: wgpu 30.0.1, winit 0.30.13, egui/eframe/egui-wgpu 0.36.1 (the only egui family on
  wgpu 30), objc2 0.6.4. eframe's `App` trait uses `logic()` + `ui()`; panels are `egui::Panel::{top,left,...}`.
- macOS: winit installs its own `NSApplicationDelegate`; `src/macos/mod.rs` adds `application:openURLs:` to that
  class at `applicationWillFinishLaunching` instead of replacing the delegate. There is no `public.step` UTI.
- GUI viewport = offscreen wgpu texture registered as an egui native texture (no `CallbackTrait`); picking reads
  the renderer's ID attachment at the cursor.
