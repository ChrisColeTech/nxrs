//! State objects, packed in place (`obj`): the game copies them by value and binds them into command buffers.

use super::entry::nvn_fns;
use super::obj::*;
use super::types::*;

unsafe fn st<T>(p: *mut impl Sized) -> &'static mut T {
    &mut *(p as *mut T)
}

nvn_fns! {
    fn nvnBlendStateSetDefaults(s: *mut NVNblendState) {
        let mut b = blend_st(0);
        b.set(blend_field::SRC_RGB, blend_func::ONE);
        b.set(blend_field::DST_RGB, blend_func::ZERO);
        b.set(blend_field::SRC_A, blend_func::ONE);
        b.set(blend_field::DST_A, blend_func::ZERO);
        b.set(blend_field::EQ_RGB, blend_eq::ADD);
        b.set(blend_field::EQ_A, blend_eq::ADD);
        *st::<blend_st>(s) = b;
    }
    fn nvnBlendStateSetBlendTarget(s: *mut NVNblendState, t: i32) { st::<blend_st>(s).set(blend_field::TARGET, t as u32); }
    fn nvnBlendStateSetBlendFunc(s: *mut NVNblendState, sr: i32, dr: i32, sa: i32, da: i32) {
        let o = st::<blend_st>(s);
        o.set(blend_field::SRC_RGB, sr as u32);
        o.set(blend_field::DST_RGB, dr as u32);
        o.set(blend_field::SRC_A, sa as u32);
        o.set(blend_field::DST_A, da as u32);
    }
    fn nvnBlendStateSetBlendEquation(s: *mut NVNblendState, er: i32, ea: i32) {
        let o = st::<blend_st>(s);
        o.set(blend_field::EQ_RGB, er as u32);
        o.set(blend_field::EQ_A, ea as u32);
    }
    fn nvnBlendStateSetAdvancedMode(s: *mut NVNblendState, m: i32) { st::<blend_st>(s).set(blend_field::ADV, if (0..=46).contains(&m) { m as u32 } else { 0 }); }
    fn nvnBlendStateGetBlendTarget(s: *const NVNblendState) -> i32 { (*(s as *const blend_st)).get(blend_field::TARGET) as i32 }
    fn nvnBlendStateGetBlendFunc(s: *const NVNblendState, sr: *mut i32, dr: *mut i32, sa: *mut i32, da: *mut i32) {
        let o = *(s as *const blend_st);
        *sr = o.get(blend_field::SRC_RGB) as i32;
        *dr = o.get(blend_field::DST_RGB) as i32;
        *sa = o.get(blend_field::SRC_A) as i32;
        *da = o.get(blend_field::DST_A) as i32;
    }
    fn nvnBlendStateGetBlendEquation(s: *const NVNblendState, er: *mut i32, ea: *mut i32) {
        let o = *(s as *const blend_st);
        *er = o.get(blend_field::EQ_RGB) as i32;
        *ea = o.get(blend_field::EQ_A) as i32;
    }
    fn nvnColorStateSetDefaults(s: *mut NVNcolorState) { *st::<color_st>(s) = color_st::new(0, logic_op::COPY, cmp::ALWAYS); }
    fn nvnColorStateSetBlendEnable(s: *mut NVNcolorState, i: i32, en: u32) {
        if !(0..=7).contains(&i) { return; }
        let o = st::<color_st>(s);
        let be = if super::b(en) { o.blend_en() | 1 << i } else { o.blend_en() & !(1 << i) };
        *o = color_st::new(be, o.logic_op(), o.alpha_func());
    }
    fn nvnColorStateSetLogicOp(s: *mut NVNcolorState, op: i32) { let o = st::<color_st>(s); *o = color_st::new(o.blend_en(), op as u32, o.alpha_func()); }
    fn nvnColorStateSetAlphaTest(s: *mut NVNcolorState, f: i32) { let o = st::<color_st>(s); *o = color_st::new(o.blend_en(), o.logic_op(), f as u32); }
    fn nvnColorStateGetBlendEnable(s: *const NVNcolorState, i: i32) -> NVNboolean { ((*(s as *const color_st)).blend_en() >> (i & 7) & 1) as u8 }
    fn nvnColorStateGetLogicOp(s: *const NVNcolorState) -> i32 { (*(s as *const color_st)).logic_op() as i32 }
    fn nvnColorStateGetAlphaTest(s: *const NVNcolorState) -> i32 { (*(s as *const color_st)).alpha_func() as i32 }
    fn nvnChannelMaskStateSetDefaults(s: *mut NVNchannelMaskState) { *st::<chmask_st>(s) = chmask_st(0xffff_ffff); }
    fn nvnChannelMaskStateSetChannelMask(s: *mut NVNchannelMaskState, i: i32, r: u32, g: u32, bl: u32, a: u32) {
        if !(0..=7).contains(&i) { return; }
        let m = super::b(r) as u32 | (super::b(g) as u32) << 1 | (super::b(bl) as u32) << 2 | (super::b(a) as u32) << 3;
        let o = st::<chmask_st>(s);
        o.0 = (o.0 & !(0xf << (4 * i))) | m << (4 * i);
    }
    fn nvnChannelMaskStateGetChannelMask(s: *const NVNchannelMaskState, i: i32, r: *mut u8, g: *mut u8, bl: *mut u8, a: *mut u8) {
        let m = (*(s as *const chmask_st)).0 >> (4 * (i & 7));
        *r = (m & 1) as u8; *g = (m >> 1 & 1) as u8; *bl = (m >> 2 & 1) as u8; *a = (m >> 3 & 1) as u8;
    }
    fn nvnMultisampleStateSetDefaults(s: *mut NVNmultisampleState) {
        let o = st::<ms_st>(s);
        *o = ms_st::default();
        o.enable = 1;
        o.a2c_dither = 1;
        // sample locations at the pixel centre (0.5, 0.5), as the driver's defaults
        o.sl = [0x88; 16];
    }
    fn nvnMultisampleStateSetMultisampleEnable(s: *mut NVNmultisampleState, e: u32) { st::<ms_st>(s).enable = super::b(e) as u8; }
    fn nvnMultisampleStateSetSamples(s: *mut NVNmultisampleState, n: i32) { st::<ms_st>(s).samples = n as u8; }
    fn nvnMultisampleStateSetAlphaToCoverageEnable(s: *mut NVNmultisampleState, e: u32) { st::<ms_st>(s).a2c = super::b(e) as u8; }
    fn nvnMultisampleStateSetAlphaToCoverageDither(s: *mut NVNmultisampleState, e: u32) { st::<ms_st>(s).a2c_dither = super::b(e) as u8; }
    fn nvnMultisampleStateGetSamples(s: *const NVNmultisampleState) -> i32 { (*(s as *const ms_st)).samples as i32 }
    fn nvnMultisampleStateGetMultisampleEnable(s: *const NVNmultisampleState) -> NVNboolean { (*(s as *const ms_st)).enable }
    fn nvnMultisampleStateGetAlphaToCoverageEnable(s: *const NVNmultisampleState) -> NVNboolean { (*(s as *const ms_st)).a2c }
    fn nvnPolygonStateSetDefaults(s: *mut NVNpolygonState) { *st::<poly_st>(s) = poly_st::new(face::NONE, front_face::CCW, polygon_mode::FILL, 0); }
    fn nvnPolygonStateSetCullFace(s: *mut NVNpolygonState, f: i32) { let o = st::<poly_st>(s); *o = poly_st::new(f as u32, o.front(), o.mode(), o.offset_en()); }
    fn nvnPolygonStateSetFrontFace(s: *mut NVNpolygonState, f: i32) { let o = st::<poly_st>(s); *o = poly_st::new(o.cull(), f as u32, o.mode(), o.offset_en()); }
    fn nvnPolygonStateSetPolygonMode(s: *mut NVNpolygonState, m: i32) { let o = st::<poly_st>(s); *o = poly_st::new(o.cull(), o.front(), m as u32, o.offset_en()); }
    fn nvnPolygonStateSetPolygonOffsetEnables(s: *mut NVNpolygonState, e: i32) { let o = st::<poly_st>(s); *o = poly_st::new(o.cull(), o.front(), o.mode(), e as u32); }
    fn nvnPolygonStateGetCullFace(s: *const NVNpolygonState) -> i32 { (*(s as *const poly_st)).cull() as i32 }
    fn nvnPolygonStateGetFrontFace(s: *const NVNpolygonState) -> i32 { (*(s as *const poly_st)).front() as i32 }
    fn nvnPolygonStateGetPolygonMode(s: *const NVNpolygonState) -> i32 { (*(s as *const poly_st)).mode() as i32 }
    fn nvnPolygonStateGetPolygonOffsetEnables(s: *const NVNpolygonState) -> i32 { (*(s as *const poly_st)).offset_en() as i32 }
    fn nvnDepthStencilStateSetDefaults(s: *mut NVNdepthStencilState) {
        let mut d = ds_st(0);
        d.set(ds_field::DW, 1);
        d.set(ds_field::DF, cmp::LESS);
        d.set(ds_field::SF_F, cmp::ALWAYS);
        d.set(ds_field::SF_B, cmp::ALWAYS);
        for f in [ds_field::FAIL_F, ds_field::ZFAIL_F, ds_field::ZPASS_F, ds_field::FAIL_B, ds_field::ZFAIL_B, ds_field::ZPASS_B] {
            d.set(f, stencil_op::KEEP);
        }
        *st::<ds_st>(s) = d;
    }
    fn nvnDepthStencilStateSetDepthTestEnable(s: *mut NVNdepthStencilState, e: u32) { st::<ds_st>(s).set(ds_field::DT, super::b(e) as u32); }
    fn nvnDepthStencilStateSetDepthWriteEnable(s: *mut NVNdepthStencilState, e: u32) { st::<ds_st>(s).set(ds_field::DW, super::b(e) as u32); }
    fn nvnDepthStencilStateSetDepthFunc(s: *mut NVNdepthStencilState, f: i32) { st::<ds_st>(s).set(ds_field::DF, f as u32); }
    fn nvnDepthStencilStateSetStencilTestEnable(s: *mut NVNdepthStencilState, e: u32) { st::<ds_st>(s).set(ds_field::ST, super::b(e) as u32); }
    fn nvnDepthStencilStateSetStencilFunc(s: *mut NVNdepthStencilState, f: i32, func: i32) {
        let o = st::<ds_st>(s);
        if f as u32 & face::FRONT != 0 { o.set(ds_field::SF_F, func as u32); }
        if f as u32 & face::BACK != 0 { o.set(ds_field::SF_B, func as u32); }
    }
    fn nvnDepthStencilStateSetStencilOp(s: *mut NVNdepthStencilState, f: i32, fail: i32, zfail: i32, zpass: i32) {
        let o = st::<ds_st>(s);
        if f as u32 & face::FRONT != 0 { o.set(ds_field::FAIL_F, fail as u32); o.set(ds_field::ZFAIL_F, zfail as u32); o.set(ds_field::ZPASS_F, zpass as u32); }
        if f as u32 & face::BACK != 0 { o.set(ds_field::FAIL_B, fail as u32); o.set(ds_field::ZFAIL_B, zfail as u32); o.set(ds_field::ZPASS_B, zpass as u32); }
    }
    fn nvnDepthStencilStateGetDepthTestEnable(s: *const NVNdepthStencilState) -> NVNboolean { (*(s as *const ds_st)).get(ds_field::DT) as u8 }
    fn nvnDepthStencilStateGetDepthWriteEnable(s: *const NVNdepthStencilState) -> NVNboolean { (*(s as *const ds_st)).get(ds_field::DW) as u8 }
    fn nvnDepthStencilStateGetDepthFunc(s: *const NVNdepthStencilState) -> i32 { (*(s as *const ds_st)).get(ds_field::DF) as i32 }
    fn nvnDepthStencilStateGetStencilTestEnable(s: *const NVNdepthStencilState) -> NVNboolean { (*(s as *const ds_st)).get(ds_field::ST) as u8 }
    fn nvnVertexAttribStateSetDefaults(s: *mut NVNvertexAttribState) { *st::<va_st>(s) = va_st(0); }
    fn nvnVertexAttribStateSetFormat(s: *mut NVNvertexAttribState, f: i32, off: isize) {
        if !(0..(1 << 19)).contains(&off) { crate::log::red(&format!("vertex attribute offset {off} exceeds the packed range")); }
        let o = st::<va_st>(s);
        *o = va_st::new(f as u32, off as u32, o.stream());
    }
    fn nvnVertexAttribStateSetStreamIndex(s: *mut NVNvertexAttribState, i: i32) { let o = st::<va_st>(s); *o = va_st::new(o.fmt(), o.offset(), i as u32); }
    fn nvnVertexAttribStateGetFormat(s: *const NVNvertexAttribState, f: *mut i32, off: *mut isize) {
        let o = *(s as *const va_st);
        if !f.is_null() { *f = o.fmt() as i32; }
        if !off.is_null() { *off = o.offset() as isize; }
    }
    fn nvnVertexAttribStateGetStreamIndex(s: *const NVNvertexAttribState) -> i32 { (*(s as *const va_st)).stream() as i32 }
    fn nvnVertexStreamStateSetDefaults(s: *mut NVNvertexStreamState) { *st::<vs_st>(s) = vs_st::default(); }
    fn nvnVertexStreamStateSetStride(s: *mut NVNvertexStreamState, v: isize) { st::<vs_st>(s).stride = v as u32; }
    fn nvnVertexStreamStateSetDivisor(s: *mut NVNvertexStreamState, d: i32) { st::<vs_st>(s).divisor = d as u32; }
    fn nvnVertexStreamStateGetStride(s: *const NVNvertexStreamState) -> isize { (*(s as *const vs_st)).stride as isize }
    fn nvnVertexStreamStateGetDivisor(s: *const NVNvertexStreamState) -> i32 { (*(s as *const vs_st)).divisor as i32 }
}
