//! GPU preview renderer: the same pipeline as [`crate::edit::render_with`],
//! run as compute shaders (wgpu). Used by the app for live previews; exports
//! always render on the CPU. The output matches the CPU render to within
//! rounding (a level or two in 8-bit), which the `gpu_matches_cpu` test
//! checks film by film.
//!
//! Everything is optional: [`Gpu::new`] returns `None` without a usable GPU,
//! and any error during a render should fall back to the CPU path.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use image::RgbImage;
use wgpu::util::DeviceExt;

use crate::develop::CameraImage;
use crate::edit::{self, Edit};
use crate::film::{self, Engine, FilmModel, FilmPreset, GrainFrame, OutputMode};

const WG: u32 = 256;
/// Longest blur the shader's parameter block holds (taps = 2r + 1 <= 128).
const MAX_RADIUS: usize = 63;

/// Per-pass parameters; layout matches `Params` in gpu.wgsl.
#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    dims: [u32; 4],
    offs: [u32; 4],
    ints: [u32; 4],
    ints2: [u32; 4],
    f: [f32; 128],
}

impl Default for Params {
    fn default() -> Self {
        Self { dims: [0; 4], offs: [0; 4], ints: [0; 4], ints2: [0; 4], f: [0.0; 128] }
    }
}

impl Params {
    fn bytes(&self) -> &[u8] {
        // SAFETY: plain-old-data, repr(C), no padding (all 4-byte fields).
        unsafe { std::slice::from_raw_parts((self as *const Self).cast::<u8>(), std::mem::size_of::<Self>()) }
    }

    fn mat(&mut self, at: usize, m: &[[f32; 3]; 3]) {
        for r in 0..3 {
            for c in 0..3 {
                self.f[at + r * 3 + c] = m[r][c];
            }
        }
    }
}

fn f32_bytes(v: &[f32]) -> &[u8] {
    // SAFETY: f32 has no invalid bit patterns or padding.
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

/// A camera image uploaded to the GPU, ready to render many times.
pub struct GpuImage {
    buffer: wgpu::Buffer,
    /// Everything but the pixels (white balance, colour matrices, ...).
    meta: CameraImage,
    width: usize,
    height: usize,
}

impl GpuImage {
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipelines: std::collections::HashMap<&'static str, wgpu::ComputePipeline>,
    max_binding: u64,
    /// Adapter name, for the app's About / log.
    pub name: String,
}

const ENTRIES: &[&str] = &[
    "working",
    "luma",
    "sharpen",
    "blur_h",
    "blur_v",
    "extract",
    "copy_plane",
    "box_down",
    "halation_source",
    "upsample_plane",
    "halation_add",
    "spread_combine",
    "layers",
    "state",
    "noise",
    "grain_add",
    "finish",
    "neutral_out",
    "film_out",
];

impl Gpu {
    /// A GPU device for previews, or `None` if there isn't a usable one.
    /// Software adapters are skipped (the CPU path is faster than those),
    /// unless `REMJET_GPU_SOFTWARE` is set (for testing).
    pub fn new() -> Option<Arc<Self>> {
        Self::with_options(std::env::var_os("REMJET_GPU_SOFTWARE").is_some()).ok().map(Arc::new)
    }

    /// `allow_software` also accepts a CPU-emulated adapter (for tests).
    pub fn with_options(allow_software: bool) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .context("no GPU adapter")?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu && !allow_software {
            bail!("only a software GPU ({})", info.name);
        }
        let limits = adapter.limits();
        if limits.max_storage_buffers_per_shader_stage < 3 || limits.max_compute_invocations_per_workgroup < WG {
            bail!("GPU {} lacks compute support", info.name);
        }
        let required_limits = wgpu::Limits {
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_buffer_size: limits.max_buffer_size,
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
            ..wgpu::Limits::downlevel_defaults()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("remjet preview"),
            required_limits,
            ..Default::default()
        }))
        .context("creating GPU device")?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("remjet look"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu.wgsl").into()),
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage(1, false),
                storage(2, true),
                storage(3, true),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = ENTRIES
            .iter()
            .map(|&e| {
                let p = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(e),
                    layout: Some(&pl),
                    module: &module,
                    entry_point: Some(e),
                    compilation_options: Default::default(),
                    cache: None,
                });
                (e, p)
            })
            .collect();
        Ok(Self {
            device,
            queue,
            layout,
            pipelines,
            max_binding: limits.max_storage_buffer_binding_size.min(limits.max_buffer_size),
            name: format!("{} ({:?})", info.name, info.backend),
        })
    }

    /// Upload a camera image (once per image; render it as often as needed).
    pub fn upload(&self, cam: &CameraImage) -> Result<GpuImage> {
        let n = cam.width * cam.height;
        if work_floats(n) as u64 * 4 > self.max_binding {
            bail!("image too large for the GPU");
        }
        let mut data = Vec::with_capacity(n * 4);
        for p in &cam.pixels {
            data.extend_from_slice(&[p[0], p[1], p[2], 0.0]);
        }
        let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera image"),
            contents: f32_bytes(&data),
            usage: wgpu::BufferUsages::STORAGE,
        });
        Ok(GpuImage { buffer, meta: cam.clone_meta(), width: cam.width, height: cam.height })
    }

    /// GPU equivalent of [`edit::render_with`] (un-rotated 8-bit sRGB).
    pub fn render(&self, img: &GpuImage, edit: &Edit, preset_override: Option<&FilmPreset>, pixel_scale: f64, seed: u64) -> Result<RgbImage> {
        let frame = GrainFrame { origin: (0, 0), stride: img.width, height: img.height };
        let cam = CameraImage { width: img.width, height: img.height, ..img.meta.clone_meta() };
        let model = edit::film_model(&cam, edit, preset_override, pixel_scale, seed)?;
        let (w, h) = (img.width, img.height);
        let n = w * h;
        let mut job = Job::new(self, img, model.as_ref(), n)?;
        let (m, _) = cam.working_matrix(edit.wb_adjust(), edit.exposure_ev)?;

        // Develop: camera RGB -> working space (A), then sharpen.
        let (a, b) = (0u32, 4 * n as u32);
        let planes: [u32; 6] = std::array::from_fn(|k| (8 * n + k * n) as u32);
        let [p0, p1, p2, p3, s0, s1] = planes;
        let out = (14 * n) as u32;
        let dims = [w as u32, h as u32, 0, 0];
        let mut p = Params { dims, offs: [0, a, 0, 0], ..Default::default() };
        p.mat(0, &m);
        job.pass("working", &p, n);
        let sigma = edit::sharpen_sigma(pixel_scale);
        if edit.sharpen > 0.0 && sigma >= 0.3 {
            job.pass("luma", &Params { dims, offs: [a, p0, 0, 0], ..Default::default() }, n);
            job.pass("copy_plane", &Params { dims, offs: [p0, p1, 0, 0], ..Default::default() }, n);
            job.blur(dims, p1, p2, sigma)?;
            let mut q = Params { dims, offs: [a, p0, p1, 0], ..Default::default() };
            q.f[0] = edit.sharpen;
            job.pass("sharpen", &q, n);
        }

        let Some(model) = model else {
            let mut q = Params { dims, offs: [a, out, 0, 0], ..Default::default() };
            q.mat(0, &crate::color::working_to_srgb().map(|r| r.map(|v| v as f32)));
            job.pass("neutral_out", &q, n);
            return job.finish(out, w, h);
        };

        // Layer exposures (B).
        let mut q = Params { dims, offs: [a, b, 0, 0], ..Default::default() };
        q.mat(0, &model.to_layers);
        job.pass("layers", &q, n);
        if let Some(spread) = &model.spread {
            for (c, &(sigma, k, wide)) in spread.iter().enumerate() {
                job.pass("extract", &Params { dims, offs: [b, p0, 0, 0], ints: [c as u32, 0, 0, 0], ..Default::default() }, n);
                job.pass("copy_plane", &Params { dims, offs: [p0, p1, 0, 0], ..Default::default() }, n);
                job.blur(dims, p1, p2, sigma)?;
                if k > 0.0 {
                    job.smooth_blur(w, h, p0, p3, p2, s0, s1, wide)?;
                }
                let mut q = Params { dims, offs: [b, p1, p3, 0], ints: [c as u32, (k > 0.0) as u32, 0, 0], ..Default::default() };
                q.f[0] = k;
                job.pass("spread_combine", &q, n);
            }
        }
        if model.hal_gain > 0.0 {
            let d = ((model.hal_sigma / 3.0).floor() as usize).max(1);
            let (sw, sh) = (w.div_ceil(d), h.div_ceil(d));
            let sdims = [w as u32, h as u32, sw as u32, sh as u32];
            let mut q = Params { dims: sdims, offs: [b, s0, 0, 0], ints: [d as u32, 0, 0, 0], ..Default::default() };
            q.f[0] = model.hal_threshold;
            job.pass("halation_source", &q, sw * sh);
            job.blur([sw as u32, sh as u32, 0, 0], s0, s1, model.hal_sigma / d as f32)?;
            let mut q = Params { dims: sdims, offs: [s0, b, 0, 0], ints: [d as u32, 0, 0, 0], ..Default::default() };
            for c in 0..3 {
                q.f[c] = model.hal_tint[c] * model.hal_gain;
            }
            job.pass("halation_add", &q, n);
        }

        // Developed state (A).
        let mut q = Params { dims, offs: [b, a, 0, 0], ..Default::default() };
        q.f[0] = model.flare;
        q.f[1] = model.log_offset;
        q.f[2] = model.grey_log_e;
        q.f[3] = model.contrast;
        q.f[4] = model.highlight_slope;
        q.f[5] = model.highlight_fit.clip;
        q.f[6] = model.highlight_fit.k;
        q.f[7] = model.highlight_fit.end_slope;
        q.f[8] = model.interimage;
        q.f[9] = film::HIGHLIGHT_KNEE;
        let luts = match &model.engine {
            Engine::Matrix { analytical, dyes } => {
                q.ints[0] = 1;
                q.mat(10, dyes);
                analytical
            }
            Engine::Spectral { develop, .. } => develop,
        };
        q.f[20] = luts[0].x0 as f32;
        q.f[21] = luts[0].step as f32;
        job.pass("state", &q, n);

        // Grain.
        if model.grain_sigma > 0.0 {
            let shared_w = model.grain_corr.sqrt();
            let own_w = (1.0 - model.grain_corr).sqrt();
            let field = |job: &mut Job, s: u64, ch: u32, weight: f32| -> Result<()> {
                let mut q = Params { dims, offs: [0, p0, 0, 0], ..Default::default() };
                q.ints = [s as u32, (s >> 32) as u32, frame.origin.0 as u32, frame.origin.1 as u32];
                q.ints2[0] = frame.stride as u32;
                job.pass("noise", &q, n);
                let gain = job.blur(dims, p0, p2, model.grain_blur)?;
                let mut q = Params { dims, offs: [a, p0, 0, 0], ints: [ch, 0, 0, 0], ..Default::default() };
                q.f[0] = model.grain_sigma;
                q.f[1] = weight;
                q.f[2] = gain;
                job.pass("grain_add", &q, n);
                Ok(())
            };
            field(&mut job, film::splitmix(seed.wrapping_add(3)), 3, shared_w)?;
            if own_w > 0.0 {
                for c in 0..3 {
                    field(&mut job, film::splitmix(seed.wrapping_add(c as u64)), c as u32, own_w)?;
                }
            }
        }

        // Spectral finish -> scanner-channel density.
        if let Engine::Spectral { core, lab_scan, .. } = &model.engine {
            let mut q = Params { dims, offs: [a, 0, 0, 0], ..Default::default() };
            q.ints[3] = job.viewer_base;
            let v = &core.viewer;
            q.f[30] = v.lo;
            q.f[31] = v.scale;
            q.mat(32, &v.to_rgb);
            match (&core.lab_scan, lab_scan, &core.printer) {
                (Some(s), true, _) => {
                    q.ints[0] = 1;
                    for c in 0..3 {
                        q.f[c] = s.gain[c] as f32;
                        q.f[3 + c] = s.dmin[c] as f32;
                        q.f[6 + c] = s.p_ref[c] as f32;
                        q.f[9 + c] = s.slope[c] as f32;
                    }
                    q.f[12] = s.k as f32;
                    q.f[13] = s.c as f32;
                    q.f[14] = crate::spectral::LAB_SCAN_DMAX as f32;
                }
                (_, _, Some(pr)) => {
                    q.ints[0] = 2;
                    for c in 0..3 {
                        q.f[c] = pr.lights[c] as f32;
                        q.f[3 + c] = pr.gain[c] as f32;
                        q.f[6 + c] = pr.dmin[c] as f32;
                    }
                    q.f[15] = pr.paper.x0 as f32;
                    q.f[16] = pr.paper.step as f32;
                    q.ints[1] = pr.paper.y.len() as u32;
                    q.ints[2] = job.paper_base;
                }
                _ => q.ints[0] = 3,
            }
            job.pass("finish", &q, n);
        }

        // Scanner or projector, then 8-bit sRGB.
        let mut q = Params { dims, offs: [a, out, 0, 0], ..Default::default() };
        for c in 0..3 {
            q.f[c] = model.balance[c].0;
            q.f[3 + c] = model.balance[c].1;
            q.f[9 + c] = model.scan_wb[c];
            q.f[22 + c] = model.proj_wb[c];
        }
        q.f[6] = model.t_white;
        q.f[7] = model.t_black;
        q.f[8] = model.inv_gamma;
        if let Some(sm) = &model.scan_matrix {
            q.ints[0] = 1;
            q.mat(12, sm);
        }
        if model.output == OutputMode::Projection {
            q.ints[1] = 1;
        }
        q.f[21] = model.proj_flare;
        q.f[25] = model.proj_falloff;
        q.ints[2] = frame.origin.0 as u32;
        q.ints[3] = frame.origin.1 as u32;
        q.ints2 = [frame.stride.max(1) as u32, frame.height.max(1) as u32, 0, 0];
        job.pass("film_out", &q, n);
        job.finish(out, w, h)
    }
}

/// Floats of working memory for an image of `n` pixels: two colour images,
/// six planes and the packed output.
fn work_floats(n: usize) -> usize {
    15 * n
}

/// One render's command list.
struct Job<'a> {
    gpu: &'a Gpu,
    encoder: wgpu::CommandEncoder,
    work: wgpu::Buffer,
    lut: wgpu::Buffer,
    cam: &'a wgpu::Buffer,
    paper_base: u32,
    viewer_base: u32,
}

impl<'a> Job<'a> {
    fn new(gpu: &'a Gpu, img: &'a GpuImage, model: Option<&FilmModel>, n: usize) -> Result<Self> {
        // Lookup tables: develop LUTs, then the paper table and the viewer's.
        let mut lut: Vec<f32> = Vec::new();
        let (mut paper_base, mut viewer_base) = (0, 0);
        if let Some(model) = model {
            let luts = match &model.engine {
                Engine::Matrix { analytical, .. } => analytical,
                Engine::Spectral { develop, .. } => develop,
            };
            for l in luts {
                if l.y.len() != film::Lut::N || (l.x0, l.step) != (luts[0].x0, luts[0].step) {
                    bail!("unexpected LUT layout");
                }
                lut.extend_from_slice(&l.y);
            }
            if let Engine::Spectral { core, .. } = &model.engine {
                if let Some(pr) = &core.printer {
                    if pr.paper.y.len() < 2 {
                        bail!("paper table too short");
                    }
                    paper_base = lut.len() as u32;
                    lut.extend(pr.paper.y.iter().flat_map(|v| v.map(|x| x as f32)));
                }
                viewer_base = lut.len() as u32;
                lut.extend(core.viewer.log_xyz.iter().flatten());
            }
        }
        if lut.is_empty() {
            lut.push(0.0);
        }
        let lut = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("look tables"),
            contents: f32_bytes(&lut),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let work = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("work"),
            size: (work_floats(n) * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        Ok(Self { gpu, encoder, work, lut, cam: &img.buffer, paper_base, viewer_base })
    }

    /// Run one entry point over `count` items.
    fn pass(&mut self, entry: &'static str, p: &Params, count: usize) {
        let dev = &self.gpu.device;
        let uniform = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: p.bytes(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.gpu.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.work.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.lut.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.cam.as_entire_binding() },
            ],
        });
        let groups = (count as u32).div_ceil(WG);
        // Rows of at most 65535 workgroups; the shader rebuilds the index.
        let (gx, gy) = if groups <= 65535 { (groups.max(1), 1) } else { (65535, groups.div_ceil(65535)) };
        let mut pass = self.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(entry), timestamp_writes: None });
        pass.set_pipeline(&self.gpu.pipelines[entry]);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(gx, gy, 1);
    }

    /// In-place [`film::blur`] of the plane at `plane` (`tmp` is scratch).
    /// Returns the noise reduction factor, as the CPU blur does.
    fn blur(&mut self, dims: [u32; 4], plane: u32, tmp: u32, sigma: f32) -> Result<f32> {
        let Some((k, gain)) = film::blur_kernel(sigma) else { return Ok(1.0) };
        let r = k.len() / 2;
        if r > MAX_RADIUS {
            bail!("blur too wide for the GPU path");
        }
        let n = (dims[0] * dims[1]) as usize;
        let mut p = Params { dims, offs: [plane, tmp, 0, 0], ints: [r as u32, 0, 0, 0], ..Default::default() };
        p.f[..k.len()].copy_from_slice(&k);
        self.pass("blur_h", &p, n);
        p.offs = [tmp, plane, 0, 0];
        self.pass("blur_v", &p, n);
        Ok(gain)
    }

    /// [`film`]'s smooth_blur: plane `src` -> `dst`, through a reduced grid.
    #[allow(clippy::too_many_arguments)]
    fn smooth_blur(&mut self, w: usize, h: usize, src: u32, dst: u32, tmp: u32, s0: u32, s1: u32, sigma: f32) -> Result<()> {
        let dims = [w as u32, h as u32, 0, 0];
        let d = ((sigma / 3.0).floor() as usize).max(1);
        if d == 1 {
            self.pass("copy_plane", &Params { dims, offs: [src, dst, 0, 0], ..Default::default() }, w * h);
            self.blur(dims, dst, tmp, sigma)?;
            return Ok(());
        }
        let (sw, sh) = (w.div_ceil(d), h.div_ceil(d));
        let sdims = [w as u32, h as u32, sw as u32, sh as u32];
        self.pass("box_down", &Params { dims: sdims, offs: [src, s0, 0, 0], ints: [d as u32, 0, 0, 0], ..Default::default() }, sw * sh);
        self.blur([sw as u32, sh as u32, 0, 0], s0, s1, sigma / d as f32)?;
        self.pass("upsample_plane", &Params { dims: sdims, offs: [s0, dst, 0, 0], ints: [d as u32, 0, 0, 0], ..Default::default() }, w * h);
        Ok(())
    }

    /// Submit, wait, and read back the packed output.
    fn finish(mut self, out: u32, w: usize, h: usize) -> Result<RgbImage> {
        let bytes = (w * h * 4) as u64;
        let read = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.encoder.copy_buffer_to_buffer(&self.work, out as u64 * 4, &read, 0, bytes);
        self.gpu.queue.submit([self.encoder.finish()]);
        let slice = read.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely()).context("waiting for the GPU")?;
        rx.recv().context("GPU readback lost")?.context("reading back from the GPU")?;
        let data = slice.get_mapped_range().map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let mut rgb = Vec::with_capacity(w * h * 3);
        for px in data.chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
        }
        drop(data);
        read.unmap();
        RgbImage::from_raw(w as u32, h as u32, rgb).context("output size")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic camera image: smooth gradients, saturated patches and a
    /// few clipped highlights, so every stage has something to do.
    fn test_image(w: usize, h: usize) -> CameraImage {
        let pixels = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                let band = ((x * 6.0) as usize) % 6;
                let sat = [[0.9, 0.1, 0.1], [0.1, 0.8, 0.1], [0.1, 0.1, 0.9], [0.8, 0.7, 0.1], [0.1, 0.6, 0.7], [0.5, 0.5, 0.5]][band];
                let tone = 2f32.powf((y - 0.7) * 9.0);
                let ripple = 1.0 + 0.3 * ((x * 90.0).sin() * (y * 70.0).cos());
                let hot = if ((x - 0.8).powi(2) + (y - 0.2).powi(2)) < 0.002 { 1.0 } else { 0.0 };
                sat.map(|c: f32| (c * tone * ripple).max(hot).min(1.0))
            })
            .collect();
        CameraImage::synthetic(w, h, pixels)
    }

    fn compare(a: &RgbImage, b: &RgbImage) -> (u8, f64) {
        let mut max = 0u8;
        let mut sum = 0u64;
        for (p, q) in a.as_raw().iter().zip(b.as_raw()) {
            let d = p.abs_diff(*q);
            max = max.max(d);
            sum += d as u64;
        }
        (max, sum as f64 / a.as_raw().len() as f64)
    }

    /// The GPU preview must match the CPU render (which exports use) on
    /// every built-in film and output. Skipped when no GPU (not even a
    /// software one, e.g. lavapipe or WARP) is available.
    #[test]
    fn gpu_matches_cpu() {
        let gpu = match Gpu::with_options(true) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e:#}");
                return;
            }
        };
        eprintln!("GPU: {}", gpu.name);
        let cam = test_image(640, 427);
        let img = gpu.upload(&cam).unwrap();
        let seed = 0x1234_5678_9abc_def0;
        let mut edits = vec![Edit { preset: None, ..Default::default() }];
        for (id, _) in crate::presets::list() {
            if !crate::presets::is_builtin(&id) {
                continue;
            }
            let base = Edit { preset: Some(id.clone()), ..Default::default() };
            edits.push(base.clone());
            edits.push(Edit { halation: 0.7, highlights: -0.5, exposure_ev: 0.6, warmth: 0.3, ..base.clone() });
            edits.push(edit::with_film(&Edit { output: OutputMode::Print, ..base.clone() }, Some(&id)));
            edits.push(edit::with_film(&Edit { output: OutputMode::Projection, scanner_colour: false, ..base.clone() }, Some(&id)));
        }
        for scale in [1.0, 4.0] {
            for e in &edits {
                let cpu = edit::render(&cam, e, scale, seed).unwrap();
                let g = gpu.render(&img, e, None, scale, seed).unwrap();
                let range = cpu.as_raw().iter().max().unwrap() - cpu.as_raw().iter().min().unwrap();
                assert!(range > 150, "test image should span most tones, got {range}");
                let (max, mean) = compare(&cpu, &g);
                eprintln!("{:?} {:?} scale {scale}: max {max} mean {mean:.4}", e.preset, e.output);
                assert!(max <= 3 && mean < 0.05, "{:?} {:?} scale {scale}: max diff {max}, mean {mean}", e.preset, e.output);
            }
        }
    }
}
