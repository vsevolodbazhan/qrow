//! Hardware Core OpenGL composition, presented through a synchronous CAOpenGLLayer.
mod atlas;
mod context;
mod engine;
mod scene;
#[cfg(test)]
mod tests;

use anyhow::Result;
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
use anyhow::anyhow;
use atlas::OpenGlAtlas;
use cgl::{CGLContextObj, CGLPixelFormatObj, CGLRetainPixelFormat};
use cocoa::base::{id, nil};
use context::Context;
use engine::Engine;
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
use gpui::PlatformAtlas;
use gpui::{DevicePixels, Scene, Size};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    rc::StrongPtr,
    runtime::{BOOL, Class, NO, Object, Sel, YES},
    sel, sel_impl,
};
use parking_lot::Mutex;
use scene::Frame;
use std::{
    ffi::c_void,
    ptr,
    sync::{Arc, OnceLock},
};

const STATE: &str = "qrowOpenGlState";
struct LayerState {
    engine: Engine,
    atlas: Arc<OpenGlAtlas>,
    frame: Option<Frame>,
    size: Size<DevicePixels>,
    opaque: bool,
    closed: bool,
    metrics: bool,
    encode_ms: f64,
}

// The ivar owns one Arc. Presentation layer copies own a separate Arc.
unsafe fn state(this: &Object) -> Option<&Mutex<LayerState>> {
    let raw: *const c_void = unsafe { *this.get_ivar(STATE) };
    let raw = raw.cast::<Mutex<LayerState>>();
    unsafe { raw.as_ref() }
}
extern "C" fn dealloc(this: &mut Object, _: Sel) {
    unsafe {
        let raw: *const c_void = *this.get_ivar(STATE);
        let raw = raw.cast::<Mutex<LayerState>>();
        this.set_ivar(STATE, ptr::null::<c_void>());
        if !raw.is_null() {
            drop(Arc::from_raw(raw));
        }
        let _: () = msg_send![super(this, class!(CAOpenGLLayer)), dealloc];
    }
}
extern "C" fn init_with_layer(this: &mut Object, _: Sel, other: id) -> id {
    unsafe {
        let result: id = msg_send![super(this,class!(CAOpenGLLayer)),initWithLayer:other];
        let is_gl: BOOL = msg_send![other,isKindOfClass:layer_class()];
        if !result.is_null() && is_gl != NO {
            let raw: *const c_void = *(*other).get_ivar(STATE);
            let raw = raw.cast::<Mutex<LayerState>>();
            if !raw.is_null() {
                Arc::increment_strong_count(raw);
                (*result).set_ivar(STATE, raw.cast::<c_void>());
            }
        }
        result
    }
}
extern "C" fn copy_format(this: &Object, _: Sel, _mask: u32) -> CGLPixelFormatObj {
    unsafe {
        state(this)
            .map(|s| CGLRetainPixelFormat(s.lock().engine.context.format))
            .unwrap_or(ptr::null_mut())
    }
}
extern "C" fn copy_context(this: &Object, _: Sel, _format: CGLPixelFormatObj) -> CGLContextObj {
    unsafe {
        state(this)
            .map(|s| context::CGLRetainContext(s.lock().engine.context.raw))
            .unwrap_or(ptr::null_mut())
    }
}
extern "C" fn release_context(_: &Object, _: Sel, context: CGLContextObj) {
    unsafe { context::CGLReleaseContext(context) };
}
extern "C" fn release_format(_: &Object, _: Sel, format: CGLPixelFormatObj) {
    unsafe { cgl::CGLReleasePixelFormat(format) };
}
extern "C" fn can_draw(
    this: &Object,
    _: Sel,
    _ctx: CGLContextObj,
    _format: CGLPixelFormatObj,
    _time: f64,
    _stamp: *const c_void,
) -> BOOL {
    let Some(state) = (unsafe { state(this) }) else {
        return NO;
    };
    let state = state.lock();
    if !state.closed && state.frame.is_some() && state.size.width.0 > 0 && state.size.height.0 > 0 {
        YES
    } else {
        NO
    }
}
extern "C" fn draw_layer(
    this: &Object,
    _: Sel,
    ctx: CGLContextObj,
    format: CGLPixelFormatObj,
    time: f64,
    stamp: *const c_void,
) {
    if let Some(state) = unsafe { state(this) } {
        let mut state = state.lock();
        if state.closed {
            return;
        }
        let LayerState {
            engine,
            atlas,
            frame,
            size,
            opaque,
            metrics,
            encode_ms,
            ..
        } = &mut *state;
        if let Some(frame) = frame {
            let started = metrics.then(std::time::Instant::now);
            let result = Context::with_raw_current(ctx, || unsafe {
                engine.draw(frame, atlas, size.width.0, size.height.0, *opaque)?;
                if *metrics {
                    use glow::HasContext;
                    // Only the explicit measurement mode waits for GPU work.
                    engine.gl.finish();
                }
                Ok(())
            });
            if let Some(started) = started {
                eprintln!(
                    "QROW_RENDER_FRAME backend=opengl time={:.6} ms={:.3}",
                    crate::renderer::metric_time(),
                    *encode_ms + started.elapsed().as_secs_f64() * 1000.
                );
            }
            if let Err(error) = result {
                eprintln!("Qrow: OpenGL drawing failed: {error:#}");
                state.closed = true;
            }
        }
    }
    // CAOpenGLLayer owns the drawable and flushes it in its superclass method.
    unsafe {
        let _: () = msg_send![super(this,class!(CAOpenGLLayer)),drawInCGLContext:ctx pixelFormat:format forLayerTime:time displayTime:stamp];
    }
}
fn layer_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| {
        let mut decl = ClassDecl::new("QrowOpenGLLayer", class!(CAOpenGLLayer))
            .expect("unique OpenGL layer class");
        decl.add_ivar::<*const c_void>(STATE);
        unsafe {
            decl.add_method(sel!(dealloc), dealloc as extern "C" fn(&mut Object, Sel));
            decl.add_method(
                sel!(initWithLayer:),
                init_with_layer as extern "C" fn(&mut Object, Sel, id) -> id,
            );
            decl.add_method(
                sel!(copyCGLPixelFormatForDisplayMask:),
                copy_format as extern "C" fn(&Object, Sel, u32) -> CGLPixelFormatObj,
            );
            decl.add_method(
                sel!(copyCGLContextForPixelFormat:),
                copy_context as extern "C" fn(&Object, Sel, CGLPixelFormatObj) -> CGLContextObj,
            );
            decl.add_method(
                sel!(releaseCGLContext:),
                release_context as extern "C" fn(&Object, Sel, CGLContextObj),
            );
            decl.add_method(
                sel!(releaseCGLPixelFormat:),
                release_format as extern "C" fn(&Object, Sel, CGLPixelFormatObj),
            );
            decl.add_method(
                sel!(canDrawInCGLContext:pixelFormat:forLayerTime:displayTime:),
                can_draw
                    as extern "C" fn(
                        &Object,
                        Sel,
                        CGLContextObj,
                        CGLPixelFormatObj,
                        f64,
                        *const c_void,
                    ) -> BOOL,
            );
            decl.add_method(
                sel!(drawInCGLContext:pixelFormat:forLayerTime:displayTime:),
                draw_layer
                    as extern "C" fn(
                        &Object,
                        Sel,
                        CGLContextObj,
                        CGLPixelFormatObj,
                        f64,
                        *const c_void,
                    ),
            );
        }
        decl.register()
    })
}

pub(crate) struct OpenGlRenderer {
    layer: StrongPtr,
    state: Arc<Mutex<LayerState>>,
    atlas: Arc<OpenGlAtlas>,
}

impl OpenGlRenderer {
    pub(crate) fn new(bounds: Size<f32>, scale: f32, transparent: bool) -> Result<Self> {
        let engine = Engine::new()?;
        eprintln!("Qrow: using OpenGL rendering ({})", engine.description);
        let atlas = Arc::new(OpenGlAtlas::default());
        let state = Arc::new(Mutex::new(LayerState {
            engine,
            atlas: atlas.clone(),
            frame: None,
            size: gpui::size(
                DevicePixels((bounds.width * scale).ceil() as i32),
                DevicePixels((bounds.height * scale).ceil() as i32),
            ),
            opaque: !transparent,
            closed: false,
            metrics: std::env::var_os("QROW_RENDER_METRICS").as_deref()
                == Some(std::ffi::OsStr::new("1")),
            encode_ms: 0.,
        }));
        let layer = unsafe {
            let layer: id = msg_send![layer_class(), new];
            (*layer).set_ivar(STATE, Arc::into_raw(state.clone()).cast::<c_void>());
            let layer = StrongPtr::new(layer);
            let _: () = msg_send![*layer,setAsynchronous:NO];
            let _: () = msg_send![*layer,setOpaque:!transparent];
            let _: () = msg_send![*layer,setContentsScale:scale as f64];
            let null: id = msg_send![class!(NSNull), null];
            let actions: id = msg_send![class!(NSDictionary),dictionaryWithObject:null forKey:crate::ns_string("contents")];
            let _: () = msg_send![*layer,setActions:actions];
            layer
        };
        Ok(Self {
            layer,
            state,
            atlas,
        })
    }
    pub(crate) fn layer_ptr(&self) -> *mut c_void {
        (*self.layer).cast()
    }
    pub(crate) fn sprite_atlas(&self) -> &Arc<OpenGlAtlas> {
        &self.atlas
    }
    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        self.state.lock().size = size;
    }
    pub(crate) fn update_transparency(&mut self, transparent: bool) {
        self.state.lock().opaque = !transparent;
        unsafe {
            let _: () = msg_send![*self.layer,setOpaque:!transparent];
        }
    }
    pub(crate) fn destroy(&self) {
        let mut state = self.state.lock();
        state.closed = true;
        state.frame = None;
    }
    pub(crate) fn draw(&mut self, scene: &Scene) {
        {
            let mut state = self.state.lock();
            let started = state.metrics.then(std::time::Instant::now);
            state.frame = Some(Frame::encode(scene));
            state.encode_ms = started.map_or(0., |started| started.elapsed().as_secs_f64() * 1000.);
        }
        unsafe {
            // AppKit installs the view as a layer delegate. CAOpenGLLayer draws
            // its own contents. Clear the delegate before invalidating it so a
            // display callback cannot re-enter the GPUI window's locked state.
            let _: () = msg_send![*self.layer,setDelegate:nil];
            let _: () = msg_send![*self.layer, setNeedsDisplay];
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn render_to_image(&mut self, scene: &Scene) -> Result<image::RgbaImage> {
        let mut state = self.state.lock();
        let size = state.size;
        let frame = Frame::encode(scene);
        render_image(&mut state.engine, &frame, &self.atlas, size)
    }
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
fn render_image(
    engine: &mut Engine,
    frame: &Frame,
    atlas: &OpenGlAtlas,
    size: Size<DevicePixels>,
) -> Result<image::RgbaImage> {
    render_target(engine, frame, atlas, size, true)?.ok_or_else(|| anyhow!("missing OpenGL image"))
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
fn render_target(
    engine: &mut Engine,
    frame: &Frame,
    atlas: &OpenGlAtlas,
    size: Size<DevicePixels>,
    readback: bool,
) -> Result<Option<image::RgbaImage>> {
    use glow::HasContext;
    let (w, h) = (size.width.0, size.height.0);
    anyhow::ensure!(
        w > 0 && h > 0 && (w as i64) * (h as i64) <= 64 * 1024 * 1024,
        "invalid OpenGL readback size"
    );
    Context::with_raw_current(engine.context.raw, || unsafe {
        let previous = engine
            .gl
            .get_parameter_framebuffer(glow::FRAMEBUFFER_BINDING);
        let texture = engine.gl.create_texture().map_err(|e| anyhow!(e))?;
        let framebuffer = match engine.gl.create_framebuffer() {
            Ok(f) => f,
            Err(e) => {
                engine.gl.delete_texture(texture);
                return Err(anyhow!(e));
            }
        };
        let result = (|| {
            engine.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            engine.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                w,
                h,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            engine
                .gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            engine.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            anyhow::ensure!(
                engine.gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE,
                "OpenGL test framebuffer incomplete"
            );
            engine.draw(frame, atlas, w, h, false)?;
            if !readback {
                return Ok(None);
            }
            let mut pixels = vec![0; w as usize * h as usize * 4];
            engine.gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            engine.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            engine.gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 0);
            engine.gl.pixel_store_i32(glow::PACK_SKIP_ROWS, 0);
            engine.gl.pixel_store_i32(glow::PACK_SKIP_PIXELS, 0);
            engine.gl.read_pixels(
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            let stride = w as usize * 4;
            for y in 0..h as usize / 2 {
                let (top, rest) = pixels.split_at_mut((h as usize - 1 - y) * stride);
                top[y * stride..(y + 1) * stride].swap_with_slice(&mut rest[..stride]);
            }
            for pixel in pixels.chunks_exact_mut(4) {
                if pixel[3] != 0 {
                    for c in 0..3 {
                        pixel[c] = ((pixel[c] as u32 * 255) / pixel[3] as u32).min(255) as u8;
                    }
                }
            }
            image::RgbaImage::from_raw(w as u32, h as u32, pixels)
                .map(Some)
                .ok_or_else(|| anyhow!("invalid OpenGL frame"))
        })();
        engine.gl.bind_framebuffer(glow::FRAMEBUFFER, previous);
        engine.gl.delete_framebuffer(framebuffer);
        engine.gl.delete_texture(texture);
        result
    })
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub(crate) struct OpenGlHeadlessRenderer {
    engine: Engine,
    atlas: Arc<OpenGlAtlas>,
}
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl OpenGlHeadlessRenderer {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            engine: Engine::new()?,
            atlas: Arc::default(),
        })
    }
}
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl gpui::PlatformHeadlessRenderer for OpenGlHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<image::RgbaImage> {
        let frame = Frame::encode(scene);
        render_image(&mut self.engine, &frame, &self.atlas, size)
    }
    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        let frame = Frame::encode(scene);
        render_target(&mut self.engine, &frame, &self.atlas, size, false).map(|_| ())
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
}
