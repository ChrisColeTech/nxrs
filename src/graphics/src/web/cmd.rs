//! Command buffers on the web device: `vkCmd*` calls kept as records, resolved into ring packets at submission
//! (`submit`). Barriers, layouts and debug labels have no WebGPU counterpart and are not kept.

use ash::vk;
use std::ffi::c_void;

/// Pipeline state that Vulkan lets a command buffer set (dynamic) or a pipeline fix (static).
#[derive(Clone, Debug)]
pub struct Dyn {
    pub topology: vk::PrimitiveTopology,
    pub restart: bool,
    pub cull: vk::CullModeFlags,
    pub front: vk::FrontFace,
    pub depth_test: bool,
    pub depth_write: bool,
    pub depth_compare: vk::CompareOp,
    pub stencil_test: bool,
    /// front, back: fail, pass, depth fail, compare
    pub stencil_ops: [(vk::StencilOp, vk::StencilOp, vk::StencilOp, vk::CompareOp); 2],
    pub stencil_read: [u32; 2],
    pub stencil_write: [u32; 2],
    pub stencil_ref: [u32; 2],
    pub bias_enable: bool,
    pub bias: [f32; 3],
    pub blend_const: [f32; 4],
    pub viewport: Option<vk::Viewport>,
    pub scissor: Option<vk::Rect2D>,
    pub discard: bool,
    pub depth_clamp: bool,
    pub samples: u32,
    pub a2c: bool,
    pub logic_op: bool,
    pub blend: [bool; 8],
    pub eq: [vk::ColorBlendEquationEXT; 8],
    pub mask: [vk::ColorComponentFlags; 8],
    pub clip_neg: bool,
    pub polygon: vk::PolygonMode,
    /// (binding, stride, per instance)
    pub vbind: Vec<(u32, u32, bool)>,
    /// (location, binding, format, offset)
    pub vattr: Vec<(u32, u32, vk::Format, u32)>,
}

impl Default for Dyn {
    fn default() -> Dyn {
        Dyn {
            topology: vk::PrimitiveTopology::TRIANGLE_LIST,
            restart: false,
            cull: vk::CullModeFlags::NONE,
            front: vk::FrontFace::COUNTER_CLOCKWISE,
            depth_test: false,
            depth_write: false,
            depth_compare: vk::CompareOp::ALWAYS,
            stencil_test: false,
            stencil_ops: [(vk::StencilOp::KEEP, vk::StencilOp::KEEP, vk::StencilOp::KEEP, vk::CompareOp::ALWAYS); 2],
            stencil_read: [0xff; 2],
            stencil_write: [0xff; 2],
            stencil_ref: [0; 2],
            bias_enable: false,
            bias: [0.0; 3],
            blend_const: [0.0; 4],
            viewport: None,
            scissor: None,
            discard: false,
            depth_clamp: false,
            samples: 1,
            a2c: false,
            logic_op: false,
            blend: [false; 8],
            eq: [vk::ColorBlendEquationEXT {
                src_color_blend_factor: vk::BlendFactor::ONE,
                dst_color_blend_factor: vk::BlendFactor::ZERO,
                color_blend_op: vk::BlendOp::ADD,
                src_alpha_blend_factor: vk::BlendFactor::ONE,
                dst_alpha_blend_factor: vk::BlendFactor::ZERO,
                alpha_blend_op: vk::BlendOp::ADD,
            }; 8],
            mask: [vk::ColorComponentFlags::RGBA; 8],
            clip_neg: false,
            polygon: vk::PolygonMode::FILL,
            vbind: Vec::new(),
            vattr: Vec::new(),
        }
    }
}

/// One dynamic state change (applied in order at resolution).
#[derive(Clone, Debug)]
pub enum Set {
    Topology(vk::PrimitiveTopology),
    Restart(bool),
    Cull(vk::CullModeFlags),
    Front(vk::FrontFace),
    DepthTest(bool),
    DepthWrite(bool),
    DepthCompare(vk::CompareOp),
    StencilTest(bool),
    StencilOp(vk::StencilFaceFlags, vk::StencilOp, vk::StencilOp, vk::StencilOp, vk::CompareOp),
    StencilRead(vk::StencilFaceFlags, u32),
    StencilWrite(vk::StencilFaceFlags, u32),
    StencilRef(vk::StencilFaceFlags, u32),
    BiasEnable(bool),
    Bias([f32; 3]),
    BlendConst([f32; 4]),
    Viewport(vk::Viewport),
    Scissor(vk::Rect2D),
    Discard(bool),
    DepthClamp(bool),
    Samples(u32),
    A2c(bool),
    LogicOp(bool),
    Blend(u32, Vec<bool>),
    Eq(u32, Vec<vk::ColorBlendEquationEXT>),
    Mask(u32, Vec<vk::ColorComponentFlags>),
    ClipNeg(bool),
    Polygon(vk::PolygonMode),
    VertexInput(Vec<(u32, u32, bool)>, Vec<(u32, u32, vk::Format, u32)>),
}

impl Dyn {
    pub fn apply(&mut self, s: &Set) {
        let faces = |f: vk::StencilFaceFlags| {
            [f.contains(vk::StencilFaceFlags::FRONT), f.contains(vk::StencilFaceFlags::BACK)]
        };
        match s.clone() {
            Set::Topology(v) => self.topology = v,
            Set::Restart(v) => self.restart = v,
            Set::Cull(v) => self.cull = v,
            Set::Front(v) => self.front = v,
            Set::DepthTest(v) => self.depth_test = v,
            Set::DepthWrite(v) => self.depth_write = v,
            Set::DepthCompare(v) => self.depth_compare = v,
            Set::StencilTest(v) => self.stencil_test = v,
            Set::StencilOp(f, a, b, c, d) => {
                for (i, on) in faces(f).into_iter().enumerate() {
                    if on {
                        self.stencil_ops[i] = (a, b, c, d);
                    }
                }
            }
            Set::StencilRead(f, v) => {
                for (i, on) in faces(f).into_iter().enumerate() {
                    if on {
                        self.stencil_read[i] = v;
                    }
                }
            }
            Set::StencilWrite(f, v) => {
                for (i, on) in faces(f).into_iter().enumerate() {
                    if on {
                        self.stencil_write[i] = v;
                    }
                }
            }
            Set::StencilRef(f, v) => {
                for (i, on) in faces(f).into_iter().enumerate() {
                    if on {
                        self.stencil_ref[i] = v;
                    }
                }
            }
            Set::BiasEnable(v) => self.bias_enable = v,
            Set::Bias(v) => self.bias = v,
            Set::BlendConst(v) => self.blend_const = v,
            Set::Viewport(v) => self.viewport = Some(v),
            Set::Scissor(v) => self.scissor = Some(v),
            Set::Discard(v) => self.discard = v,
            Set::DepthClamp(v) => self.depth_clamp = v,
            Set::Samples(v) => self.samples = v,
            Set::A2c(v) => self.a2c = v,
            Set::LogicOp(v) => self.logic_op = v,
            Set::Blend(first, v) => {
                for (i, x) in v.into_iter().enumerate() {
                    if let Some(s) = self.blend.get_mut(first as usize + i) {
                        *s = x;
                    }
                }
            }
            Set::Eq(first, v) => {
                for (i, x) in v.into_iter().enumerate() {
                    if let Some(s) = self.eq.get_mut(first as usize + i) {
                        *s = x;
                    }
                }
            }
            Set::Mask(first, v) => {
                for (i, x) in v.into_iter().enumerate() {
                    if let Some(s) = self.mask.get_mut(first as usize + i) {
                        *s = x;
                    }
                }
            }
            Set::ClipNeg(v) => self.clip_neg = v,
            Set::Polygon(v) => self.polygon = v,
            Set::VertexInput(b, a) => {
                self.vbind = b;
                self.vattr = a;
            }
        }
    }
}

#[derive(Clone)]
pub struct Attachment {
    pub view: u64,
    pub resolve: u64,
    pub load: vk::AttachmentLoadOp,
    pub store: vk::AttachmentStoreOp,
    pub clear: vk::ClearValue,
}

#[derive(Clone)]
pub enum Cmd {
    BeginRendering { colors: Vec<Attachment>, depth: Option<Attachment>, stencil: Option<Attachment>, area: vk::Rect2D },
    EndRendering,
    BindPipeline { compute: bool, p: u64 },
    BindSets { compute: bool, first: u32, sets: Vec<u64>, dynoff: Vec<u32> },
    Push { off: u32, data: Vec<u8> },
    Set(Set),
    BindVb { first: u32, bufs: Vec<(u64, u64, u64, u64)> },
    BindIb { buf: u64, off: u64, ty: vk::IndexType },
    Draw { vc: u32, ic: u32, fv: u32, fi: u32 },
    DrawIndexed { ic: u32, inst: u32, fi: u32, vo: i32, finst: u32 },
    DrawIndirect { buf: u64, off: u64, count: u32, stride: u32, indexed: bool },
    Dispatch { x: u32, y: u32, z: u32 },
    DispatchIndirect { buf: u64, off: u64 },
    CopyBuffer { src: u64, dst: u64, regions: Vec<vk::BufferCopy> },
    CopyBufferToImage { src: u64, dst: u64, regions: Vec<vk::BufferImageCopy> },
    CopyImageToBuffer { src: u64, dst: u64, regions: Vec<vk::BufferImageCopy> },
    CopyImage { src: u64, dst: u64, regions: Vec<vk::ImageCopy> },
    Blit { src: u64, dst: u64, regions: Vec<vk::ImageBlit>, filter: vk::Filter },
    ClearColor { image: u64, color: vk::ClearColorValue, ranges: Vec<vk::ImageSubresourceRange> },
    ClearDepth { image: u64, value: vk::ClearDepthStencilValue, ranges: Vec<vk::ImageSubresourceRange> },
    ClearAttachments { atts: Vec<vk::ClearAttachment>, rects: Vec<vk::ClearRect> },
    Fill { buf: u64, off: u64, size: u64, value: u32 },
    Update { buf: u64, off: u64, data: Vec<u8> },
    /// queries the web device does not implement (red line once at resolution)
    Unsupported(&'static str),
}

#[derive(Default)]
pub struct CmdBuf {
    pub cmds: Vec<Cmd>,
}

/// The command buffer behind a handle.
///
/// # Safety
/// `cb` was made by `vkAllocateCommandBuffers` on this device and is not freed.
pub unsafe fn cb<'a>(cb: vk::CommandBuffer) -> &'a mut CmdBuf {
    &mut *(vk::Handle::as_raw(cb) as *mut CmdBuf)
}

pub fn new_cb() -> vk::CommandBuffer {
    let b = Box::new(CmdBuf::default());
    <vk::CommandBuffer as vk::Handle>::from_raw(Box::into_raw(b) as u64)
}

/// # Safety
/// As `cb`; the handle is not used again.
pub unsafe fn free_cb(c: vk::CommandBuffer) {
    drop(Box::from_raw(vk::Handle::as_raw(c) as *mut CmdBuf));
}

unsafe fn sl<'a, T>(p: *const T, n: u32) -> &'a [T] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}

macro_rules! rec {
    ($c:expr, $v:expr) => {
        cb($c).cmds.push($v)
    };
}
macro_rules! set {
    ($c:expr, $v:expr) => {
        cb($c).cmds.push(Cmd::Set($v))
    };
}

pub unsafe extern "system" fn begin(c: vk::CommandBuffer, _i: *const vk::CommandBufferBeginInfo<'_>) -> vk::Result {
    cb(c).cmds.clear();
    vk::Result::SUCCESS
}
pub unsafe extern "system" fn end(_c: vk::CommandBuffer) -> vk::Result {
    vk::Result::SUCCESS
}
pub unsafe extern "system" fn reset(c: vk::CommandBuffer, _f: vk::CommandBufferResetFlags) -> vk::Result {
    cb(c).cmds.clear();
    vk::Result::SUCCESS
}

fn att(a: &vk::RenderingAttachmentInfo<'_>) -> Option<Attachment> {
    (a.image_view != vk::ImageView::null()).then(|| Attachment {
        view: vk::Handle::as_raw(a.image_view),
        resolve: vk::Handle::as_raw(a.resolve_image_view),
        load: a.load_op,
        store: a.store_op,
        clear: a.clear_value,
    })
}

pub unsafe extern "system" fn begin_rendering(c: vk::CommandBuffer, i: *const vk::RenderingInfo<'_>) {
    let i = &*i;
    let colors = sl(i.p_color_attachments, i.color_attachment_count)
        .iter()
        .map(|a| {
            att(a).unwrap_or(Attachment {
                view: 0,
                resolve: 0,
                load: a.load_op,
                store: a.store_op,
                clear: a.clear_value,
            })
        })
        .collect();
    let depth = i.p_depth_attachment.as_ref().and_then(att);
    let stencil = i.p_stencil_attachment.as_ref().and_then(att);
    rec!(c, Cmd::BeginRendering { colors, depth, stencil, area: i.render_area });
}
pub unsafe extern "system" fn end_rendering(c: vk::CommandBuffer) {
    rec!(c, Cmd::EndRendering);
}
pub unsafe extern "system" fn bind_pipeline(c: vk::CommandBuffer, bp: vk::PipelineBindPoint, p: vk::Pipeline) {
    rec!(c, Cmd::BindPipeline { compute: bp == vk::PipelineBindPoint::COMPUTE, p: vk::Handle::as_raw(p) });
}
pub unsafe extern "system" fn bind_descriptor_sets(
    c: vk::CommandBuffer,
    bp: vk::PipelineBindPoint,
    _l: vk::PipelineLayout,
    first: u32,
    n: u32,
    sets: *const vk::DescriptorSet,
    nd: u32,
    dynoff: *const u32,
) {
    rec!(
        c,
        Cmd::BindSets {
            compute: bp == vk::PipelineBindPoint::COMPUTE,
            first,
            sets: sl(sets, n).iter().map(|s| vk::Handle::as_raw(*s)).collect(),
            dynoff: sl(dynoff, nd).to_vec(),
        }
    );
}
pub unsafe extern "system" fn push_constants(
    c: vk::CommandBuffer,
    _l: vk::PipelineLayout,
    _s: vk::ShaderStageFlags,
    off: u32,
    size: u32,
    p: *const c_void,
) {
    rec!(c, Cmd::Push { off, data: sl(p as *const u8, size).to_vec() });
}
pub unsafe extern "system" fn bind_vertex_buffers2(
    c: vk::CommandBuffer,
    first: u32,
    n: u32,
    bufs: *const vk::Buffer,
    offs: *const vk::DeviceSize,
    sizes: *const vk::DeviceSize,
    strides: *const vk::DeviceSize,
) {
    let b = sl(bufs, n);
    let o = sl(offs, n);
    let s = sl(sizes, n);
    let st = sl(strides, n);
    let v = (0..n as usize)
        .map(|i| {
            (
                vk::Handle::as_raw(b[i]),
                o.get(i).copied().unwrap_or(0),
                s.get(i).copied().unwrap_or(vk::WHOLE_SIZE),
                st.get(i).copied().unwrap_or(0),
            )
        })
        .collect();
    rec!(c, Cmd::BindVb { first, bufs: v });
}
pub unsafe extern "system" fn bind_vertex_buffers(
    c: vk::CommandBuffer,
    first: u32,
    n: u32,
    bufs: *const vk::Buffer,
    offs: *const vk::DeviceSize,
) {
    bind_vertex_buffers2(c, first, n, bufs, offs, std::ptr::null(), std::ptr::null());
}
pub unsafe extern "system" fn bind_index_buffer(c: vk::CommandBuffer, b: vk::Buffer, off: u64, ty: vk::IndexType) {
    rec!(c, Cmd::BindIb { buf: vk::Handle::as_raw(b), off, ty });
}
pub unsafe extern "system" fn draw(c: vk::CommandBuffer, vc: u32, ic: u32, fv: u32, fi: u32) {
    rec!(c, Cmd::Draw { vc, ic, fv, fi });
}
pub unsafe extern "system" fn draw_indexed(c: vk::CommandBuffer, ic: u32, inst: u32, fi: u32, vo: i32, finst: u32) {
    rec!(c, Cmd::DrawIndexed { ic, inst, fi, vo, finst });
}
pub unsafe extern "system" fn draw_indirect(c: vk::CommandBuffer, b: vk::Buffer, off: u64, count: u32, stride: u32) {
    rec!(c, Cmd::DrawIndirect { buf: vk::Handle::as_raw(b), off, count, stride, indexed: false });
}
pub unsafe extern "system" fn draw_indexed_indirect(c: vk::CommandBuffer, b: vk::Buffer, off: u64, count: u32, stride: u32) {
    rec!(c, Cmd::DrawIndirect { buf: vk::Handle::as_raw(b), off, count, stride, indexed: true });
}
pub unsafe extern "system" fn draw_indirect_count(
    c: vk::CommandBuffer,
    _b: vk::Buffer,
    _o: u64,
    _cb: vk::Buffer,
    _co: u64,
    _m: u32,
    _s: u32,
) {
    rec!(c, Cmd::Unsupported("vkCmdDraw*IndirectCount (WebGPU has no indirect count)"));
}
pub unsafe extern "system" fn dispatch(c: vk::CommandBuffer, x: u32, y: u32, z: u32) {
    rec!(c, Cmd::Dispatch { x, y, z });
}
pub unsafe extern "system" fn dispatch_indirect(c: vk::CommandBuffer, b: vk::Buffer, off: u64) {
    rec!(c, Cmd::DispatchIndirect { buf: vk::Handle::as_raw(b), off });
}
pub unsafe extern "system" fn copy_buffer(c: vk::CommandBuffer, s: vk::Buffer, d: vk::Buffer, n: u32, r: *const vk::BufferCopy) {
    rec!(c, Cmd::CopyBuffer { src: vk::Handle::as_raw(s), dst: vk::Handle::as_raw(d), regions: sl(r, n).to_vec() });
}
pub unsafe extern "system" fn copy_buffer_to_image(
    c: vk::CommandBuffer,
    s: vk::Buffer,
    d: vk::Image,
    _l: vk::ImageLayout,
    n: u32,
    r: *const vk::BufferImageCopy,
) {
    rec!(c, Cmd::CopyBufferToImage { src: vk::Handle::as_raw(s), dst: vk::Handle::as_raw(d), regions: sl(r, n).to_vec() });
}
pub unsafe extern "system" fn copy_image_to_buffer(
    c: vk::CommandBuffer,
    s: vk::Image,
    _l: vk::ImageLayout,
    d: vk::Buffer,
    n: u32,
    r: *const vk::BufferImageCopy,
) {
    rec!(c, Cmd::CopyImageToBuffer { src: vk::Handle::as_raw(s), dst: vk::Handle::as_raw(d), regions: sl(r, n).to_vec() });
}
pub unsafe extern "system" fn copy_image(
    c: vk::CommandBuffer,
    s: vk::Image,
    _sl: vk::ImageLayout,
    d: vk::Image,
    _dl: vk::ImageLayout,
    n: u32,
    r: *const vk::ImageCopy,
) {
    rec!(c, Cmd::CopyImage { src: vk::Handle::as_raw(s), dst: vk::Handle::as_raw(d), regions: sl(r, n).to_vec() });
}
pub unsafe extern "system" fn blit_image(
    c: vk::CommandBuffer,
    s: vk::Image,
    _sl: vk::ImageLayout,
    d: vk::Image,
    _dl: vk::ImageLayout,
    n: u32,
    r: *const vk::ImageBlit,
    filter: vk::Filter,
) {
    rec!(c, Cmd::Blit { src: vk::Handle::as_raw(s), dst: vk::Handle::as_raw(d), regions: sl(r, n).to_vec(), filter });
}
pub unsafe extern "system" fn clear_color_image(
    c: vk::CommandBuffer,
    i: vk::Image,
    _l: vk::ImageLayout,
    color: *const vk::ClearColorValue,
    n: u32,
    r: *const vk::ImageSubresourceRange,
) {
    rec!(c, Cmd::ClearColor { image: vk::Handle::as_raw(i), color: *color, ranges: sl(r, n).to_vec() });
}
pub unsafe extern "system" fn clear_depth_stencil_image(
    c: vk::CommandBuffer,
    i: vk::Image,
    _l: vk::ImageLayout,
    v: *const vk::ClearDepthStencilValue,
    n: u32,
    r: *const vk::ImageSubresourceRange,
) {
    rec!(c, Cmd::ClearDepth { image: vk::Handle::as_raw(i), value: *v, ranges: sl(r, n).to_vec() });
}
pub unsafe extern "system" fn clear_attachments(
    c: vk::CommandBuffer,
    na: u32,
    a: *const vk::ClearAttachment,
    nr: u32,
    r: *const vk::ClearRect,
) {
    rec!(c, Cmd::ClearAttachments { atts: sl(a, na).to_vec(), rects: sl(r, nr).to_vec() });
}
pub unsafe extern "system" fn fill_buffer(c: vk::CommandBuffer, b: vk::Buffer, off: u64, size: u64, value: u32) {
    rec!(c, Cmd::Fill { buf: vk::Handle::as_raw(b), off, size, value });
}
pub unsafe extern "system" fn update_buffer(c: vk::CommandBuffer, b: vk::Buffer, off: u64, size: u64, p: *const c_void) {
    rec!(c, Cmd::Update { buf: vk::Handle::as_raw(b), off, data: sl(p as *const u8, size as u32).to_vec() });
}

// ---------- copies and blits, the "2" forms (VK 1.3) ----------

pub unsafe extern "system" fn copy_buffer2(c: vk::CommandBuffer, i: *const vk::CopyBufferInfo2<'_>) {
    let i = &*i;
    let regions = sl(i.p_regions, i.region_count)
        .iter()
        .map(|r| vk::BufferCopy { src_offset: r.src_offset, dst_offset: r.dst_offset, size: r.size })
        .collect();
    rec!(c, Cmd::CopyBuffer { src: vk::Handle::as_raw(i.src_buffer), dst: vk::Handle::as_raw(i.dst_buffer), regions });
}
fn bic(r: &vk::BufferImageCopy2<'_>) -> vk::BufferImageCopy {
    vk::BufferImageCopy {
        buffer_offset: r.buffer_offset,
        buffer_row_length: r.buffer_row_length,
        buffer_image_height: r.buffer_image_height,
        image_subresource: r.image_subresource,
        image_offset: r.image_offset,
        image_extent: r.image_extent,
    }
}
pub unsafe extern "system" fn copy_buffer_to_image2(c: vk::CommandBuffer, i: *const vk::CopyBufferToImageInfo2<'_>) {
    let i = &*i;
    let regions = sl(i.p_regions, i.region_count).iter().map(bic).collect();
    rec!(c, Cmd::CopyBufferToImage { src: vk::Handle::as_raw(i.src_buffer), dst: vk::Handle::as_raw(i.dst_image), regions });
}
pub unsafe extern "system" fn copy_image_to_buffer2(c: vk::CommandBuffer, i: *const vk::CopyImageToBufferInfo2<'_>) {
    let i = &*i;
    let regions = sl(i.p_regions, i.region_count).iter().map(bic).collect();
    rec!(c, Cmd::CopyImageToBuffer { src: vk::Handle::as_raw(i.src_image), dst: vk::Handle::as_raw(i.dst_buffer), regions });
}
pub unsafe extern "system" fn copy_image2(c: vk::CommandBuffer, i: *const vk::CopyImageInfo2<'_>) {
    let i = &*i;
    let regions = sl(i.p_regions, i.region_count)
        .iter()
        .map(|r| vk::ImageCopy {
            src_subresource: r.src_subresource,
            src_offset: r.src_offset,
            dst_subresource: r.dst_subresource,
            dst_offset: r.dst_offset,
            extent: r.extent,
        })
        .collect();
    rec!(c, Cmd::CopyImage { src: vk::Handle::as_raw(i.src_image), dst: vk::Handle::as_raw(i.dst_image), regions });
}
pub unsafe extern "system" fn blit_image2(c: vk::CommandBuffer, i: *const vk::BlitImageInfo2<'_>) {
    let i = &*i;
    let regions = sl(i.p_regions, i.region_count)
        .iter()
        .map(|r| vk::ImageBlit {
            src_subresource: r.src_subresource,
            src_offsets: r.src_offsets,
            dst_subresource: r.dst_subresource,
            dst_offsets: r.dst_offsets,
        })
        .collect();
    rec!(c, Cmd::Blit { src: vk::Handle::as_raw(i.src_image), dst: vk::Handle::as_raw(i.dst_image), regions, filter: i.filter });
}

// ---------- no-ops: barriers, events, labels ----------

pub unsafe extern "system" fn nop_barrier2(_c: vk::CommandBuffer, _d: *const vk::DependencyInfo<'_>) {}
#[allow(clippy::too_many_arguments)]
pub unsafe extern "system" fn nop_barrier(
    _c: vk::CommandBuffer,
    _s: vk::PipelineStageFlags,
    _d: vk::PipelineStageFlags,
    _f: vk::DependencyFlags,
    _a: u32,
    _b: *const c_void,
    _e: u32,
    _g: *const c_void,
    _h: u32,
    _i: *const c_void,
) {
}
pub unsafe extern "system" fn nop_label(_c: vk::CommandBuffer, _l: *const c_void) {}
pub unsafe extern "system" fn nop_end_label(_c: vk::CommandBuffer) {}

// ---------- queries ----------

pub unsafe extern "system" fn write_timestamp2(_c: vk::CommandBuffer, _s: vk::PipelineStageFlags2, _q: vk::QueryPool, _i: u32) {}
pub unsafe extern "system" fn reset_query_pool(_c: vk::CommandBuffer, _q: vk::QueryPool, _f: u32, _n: u32) {}
pub unsafe extern "system" fn begin_query(c: vk::CommandBuffer, _q: vk::QueryPool, _i: u32, _f: vk::QueryControlFlags) {
    rec!(c, Cmd::Unsupported("occlusion queries"));
}
pub unsafe extern "system" fn end_query(_c: vk::CommandBuffer, _q: vk::QueryPool, _i: u32) {}

// ---------- dynamic state ----------

pub unsafe extern "system" fn set_viewport(c: vk::CommandBuffer, first: u32, n: u32, v: *const vk::Viewport) {
    if first == 0 && n > 0 {
        set!(c, Set::Viewport(*v));
    }
}
pub unsafe extern "system" fn set_viewport_with_count(c: vk::CommandBuffer, n: u32, v: *const vk::Viewport) {
    if n > 0 {
        set!(c, Set::Viewport(*v));
    }
}
pub unsafe extern "system" fn set_scissor(c: vk::CommandBuffer, first: u32, n: u32, v: *const vk::Rect2D) {
    if first == 0 && n > 0 {
        set!(c, Set::Scissor(*v));
    }
}
pub unsafe extern "system" fn set_scissor_with_count(c: vk::CommandBuffer, n: u32, v: *const vk::Rect2D) {
    if n > 0 {
        set!(c, Set::Scissor(*v));
    }
}
pub unsafe extern "system" fn set_topology(c: vk::CommandBuffer, t: vk::PrimitiveTopology) {
    set!(c, Set::Topology(t));
}
pub unsafe extern "system" fn set_restart(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::Restart(on != 0));
}
pub unsafe extern "system" fn set_cull(c: vk::CommandBuffer, m: vk::CullModeFlags) {
    set!(c, Set::Cull(m));
}
pub unsafe extern "system" fn set_front(c: vk::CommandBuffer, f: vk::FrontFace) {
    set!(c, Set::Front(f));
}
pub unsafe extern "system" fn set_depth_test(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::DepthTest(on != 0));
}
pub unsafe extern "system" fn set_depth_write(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::DepthWrite(on != 0));
}
pub unsafe extern "system" fn set_depth_compare(c: vk::CommandBuffer, op: vk::CompareOp) {
    set!(c, Set::DepthCompare(op));
}
pub unsafe extern "system" fn set_depth_bounds_test(_c: vk::CommandBuffer, _on: vk::Bool32) {}
pub unsafe extern "system" fn set_depth_bounds(_c: vk::CommandBuffer, _a: f32, _b: f32) {}
pub unsafe extern "system" fn set_stencil_test(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::StencilTest(on != 0));
}
pub unsafe extern "system" fn set_stencil_op(
    c: vk::CommandBuffer,
    f: vk::StencilFaceFlags,
    fail: vk::StencilOp,
    pass: vk::StencilOp,
    dfail: vk::StencilOp,
    cmp: vk::CompareOp,
) {
    set!(c, Set::StencilOp(f, fail, pass, dfail, cmp));
}
pub unsafe extern "system" fn set_stencil_compare_mask(c: vk::CommandBuffer, f: vk::StencilFaceFlags, m: u32) {
    set!(c, Set::StencilRead(f, m));
}
pub unsafe extern "system" fn set_stencil_write_mask(c: vk::CommandBuffer, f: vk::StencilFaceFlags, m: u32) {
    set!(c, Set::StencilWrite(f, m));
}
pub unsafe extern "system" fn set_stencil_reference(c: vk::CommandBuffer, f: vk::StencilFaceFlags, r: u32) {
    set!(c, Set::StencilRef(f, r));
}
pub unsafe extern "system" fn set_depth_bias_enable(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::BiasEnable(on != 0));
}
pub unsafe extern "system" fn set_depth_bias(c: vk::CommandBuffer, k: f32, clamp: f32, slope: f32) {
    set!(c, Set::Bias([k, clamp, slope]));
}
pub unsafe extern "system" fn set_blend_constants(c: vk::CommandBuffer, v: *const [f32; 4]) {
    set!(c, Set::BlendConst(*v));
}
pub unsafe extern "system" fn set_line_width(_c: vk::CommandBuffer, _w: f32) {}
pub unsafe extern "system" fn set_rasterizer_discard(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::Discard(on != 0));
}
pub unsafe extern "system" fn set_depth_clamp(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::DepthClamp(on != 0));
}
pub unsafe extern "system" fn set_polygon_mode(c: vk::CommandBuffer, m: vk::PolygonMode) {
    set!(c, Set::Polygon(m));
}
pub unsafe extern "system" fn set_samples(c: vk::CommandBuffer, s: vk::SampleCountFlags) {
    set!(c, Set::Samples(s.as_raw().max(1)));
}
pub unsafe extern "system" fn set_sample_mask(_c: vk::CommandBuffer, _s: vk::SampleCountFlags, _m: *const u32) {}
pub unsafe extern "system" fn set_a2c(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::A2c(on != 0));
}
pub unsafe extern "system" fn set_logic_op_enable(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::LogicOp(on != 0));
}
pub unsafe extern "system" fn set_logic_op(_c: vk::CommandBuffer, _o: vk::LogicOp) {}
pub unsafe extern "system" fn set_blend_enable(c: vk::CommandBuffer, first: u32, n: u32, v: *const vk::Bool32) {
    set!(c, Set::Blend(first, sl(v, n).iter().map(|&b| b != 0).collect()));
}
pub unsafe extern "system" fn set_blend_equation(c: vk::CommandBuffer, first: u32, n: u32, v: *const vk::ColorBlendEquationEXT) {
    set!(c, Set::Eq(first, sl(v, n).to_vec()));
}
pub unsafe extern "system" fn set_write_mask(c: vk::CommandBuffer, first: u32, n: u32, v: *const vk::ColorComponentFlags) {
    set!(c, Set::Mask(first, sl(v, n).to_vec()));
}
pub unsafe extern "system" fn set_clip_neg(c: vk::CommandBuffer, on: vk::Bool32) {
    set!(c, Set::ClipNeg(on != 0));
}
pub unsafe extern "system" fn set_patch_points(_c: vk::CommandBuffer, _n: u32) {}
pub unsafe extern "system" fn set_conservative(_c: vk::CommandBuffer, _m: vk::ConservativeRasterizationModeEXT) {}
pub unsafe extern "system" fn set_vertex_input(
    c: vk::CommandBuffer,
    nb: u32,
    b: *const vk::VertexInputBindingDescription2EXT<'_>,
    na: u32,
    a: *const vk::VertexInputAttributeDescription2EXT<'_>,
) {
    let binds = sl(b, nb)
        .iter()
        .map(|b| (b.binding, b.stride, b.input_rate == vk::VertexInputRate::INSTANCE))
        .collect();
    let attrs = sl(a, na).iter().map(|a| (a.location, a.binding, a.format, a.offset)).collect();
    set!(c, Set::VertexInput(binds, attrs));
}
