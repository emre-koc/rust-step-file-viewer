//! Internally managed render attachments: MSAA colour, depth, and the single-sampled ID pair.
//!
//! The caller owns the *resolved* colour view; everything else lives here and is recreated whenever
//! the size, sample count or colour format changes.

use crate::{DEPTH_FORMAT, ID_FORMAT};

pub(crate) struct Targets {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub format: wgpu::TextureFormat,

    /// Multisampled colour, `None` when `samples == 1` (we render straight into the caller's view).
    pub msaa_color: Option<wgpu::TextureView>,
    /// Depth for the shaded/edge passes, at `samples`.
    pub depth: wgpu::TextureView,

    /// `Rgba32Uint`: instance id + 1, shape-global face index, NDC depth. Always single-sampled.
    pub id_texture: wgpu::Texture,
    pub id_view: wgpu::TextureView,
    /// Single-sampled depth used only to depth-test the ID pass; never read back.
    pub id_depth_view: wgpu::TextureView,
}

impl Targets {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, samples: u32, format: wgpu::TextureFormat) -> Targets {
        let width = width.max(1);
        let height = height.max(1);
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };

        let msaa_color = (samples > 1).then(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("step-render msaa colour"),
                    size,
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        });

        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("step-render depth"),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        let id_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("step-render id"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ID_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let id_view = id_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let id_depth_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("step-render id depth"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        Targets { width, height, samples, format, msaa_color, depth, id_texture, id_view, id_depth_view }
    }

    pub fn matches(&self, width: u32, height: u32, samples: u32, format: wgpu::TextureFormat) -> bool {
        self.width == width.max(1) && self.height == height.max(1) && self.samples == samples && self.format == format
    }
}
