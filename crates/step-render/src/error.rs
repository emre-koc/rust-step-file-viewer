//! Renderer error type. Everything that can fail talks to the GPU; nothing here is a geometry error.

/// Failures raised by [`crate::Renderer`].
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// No GPU adapter could be found (headless CI, no Metal/Vulkan driver, …).
    #[error("no suitable GPU adapter: {0}")]
    NoAdapter(String),
    /// An adapter was found but a device could not be created from it.
    #[error("could not create GPU device: {0}")]
    NoDevice(String),
    /// `Device::poll` reported an error while waiting for a readback.
    #[error("device poll failed: {0}")]
    Poll(String),
    /// `map_async` failed or the mapped range could not be taken.
    #[error("buffer readback failed: {0}")]
    Readback(String),
    /// [`crate::Renderer::render_offscreen`] cannot convert this colour format to RGBA8.
    #[error("colour format {0:?} cannot be read back as RGBA8")]
    UnsupportedFormat(wgpu::TextureFormat),
}
