//! CPU compositing of GPUI scenes, presented as a retained Core Animation image.
//! Glyph shaping and rasterization still use the platform's Core Text backend.
use anyhow::{Result, anyhow, ensure};
use cocoa::base::{id, nil};
use core_graphics::{color_space::CGColorSpace, data_provider::CGDataProvider, image::CGImage};
use foreign_types::ForeignType;
use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTile, Background,
    BorderStyle, Bounds, ContentMask, Corners, DevicePixels, Hsla, PlatformAtlas, Point,
    PrimitiveBatch, Rgba, ScaledPixels, Scene, Size, TileId, TransformationMatrix,
};
use objc::{class, msg_send, rc::StrongPtr, sel, sel_impl};
use parking_lot::Mutex;
use std::{borrow::Cow, collections::HashMap, ffi::c_void, sync::Arc};

#[derive(Default)]
pub(crate) struct SoftwareAtlas(Mutex<AtlasState<AtlasPixels>>);

#[derive(Default)]
struct AtlasPixels {
    next_id: u32,
    tiles: HashMap<u32, TilePixels>,
}

struct TilePixels {
    width: usize,
    height: usize,
    kind: AtlasTextureKind,
    bytes: Vec<u8>,
}

impl AtlasBackend for AtlasPixels {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        ensure!(
            size.width.0 > 0 && size.height.0 > 0,
            "empty software atlas tile"
        );
        let channels = match kind {
            AtlasTextureKind::Monochrome => 1,
            _ => 4,
        };
        let width = size.width.0 as usize;
        let height = size.height.0 as usize;
        let required = width
            .checked_mul(height)
            .and_then(|n| n.checked_mul(channels))
            .ok_or_else(|| anyhow!("software atlas tile too large"))?;
        ensure!(
            bytes.len() == required,
            "invalid software atlas pixel count"
        );
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("software atlas exhausted"))?;
        let index = self.next_id;
        self.tiles.insert(
            index,
            TilePixels {
                width,
                height,
                kind,
                bytes: bytes.to_vec(),
            },
        );
        Ok(AtlasTile {
            texture_id: AtlasTextureId { index, kind },
            tile_id: TileId(index),
            padding: 0,
            bounds: Bounds {
                origin: Point::default(),
                size,
            },
        })
    }

    fn remove(&mut self, tile: AtlasTile) {
        self.tiles.remove(&tile.texture_id.index);
    }
}

impl PlatformAtlas for SoftwareAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        self.0.lock().get_or_insert_with(key, build)
    }

    fn remove(&self, key: &AtlasKey) {
        self.0.lock().remove(key);
    }

    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    fn contains(&self, key: &AtlasKey) -> bool {
        self.0.lock().contains(key)
    }
}

pub(crate) struct SoftwareRenderer {
    layer: StrongPtr,
    atlas: Arc<SoftwareAtlas>,
    size: Size<DevicePixels>,
    opaque: bool,
    metrics: bool,
}

impl SoftwareRenderer {
    pub(crate) unsafe fn new(bounds: Size<f32>, scale: f32, transparent: bool) -> Self {
        let layer: id = unsafe { msg_send![class!(CALayer), new] };
        let this = Self {
            layer: unsafe { StrongPtr::new(layer) },
            atlas: Arc::default(),
            size: gpui::size(
                DevicePixels((bounds.width * scale).ceil() as i32),
                DevicePixels((bounds.height * scale).ceil() as i32),
            ),
            opaque: !transparent,
            metrics: std::env::var_os("QROW_RENDER_METRICS").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        };
        // Disable implicit content transitions: each GPUI frame is a complete image.
        unsafe {
            let null: id = msg_send![class!(NSNull), null];
            let actions: id = msg_send![class!(NSDictionary), dictionaryWithObject: null forKey: crate::ns_string("contents")];
            let _: () = msg_send![*this.layer, setActions: actions];
            let _: () = msg_send![*this.layer, setOpaque: this.opaque];
        }
        this
    }

    pub(crate) fn layer_ptr(&self) -> *mut c_void {
        (*self.layer).cast()
    }
    pub(crate) fn sprite_atlas(&self) -> &Arc<SoftwareAtlas> {
        &self.atlas
    }
    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        self.size = size;
    }
    pub(crate) fn update_transparency(&mut self, transparent: bool) {
        self.opaque = !transparent;
        unsafe {
            let _: () = msg_send![*self.layer, setOpaque: self.opaque];
        }
    }
    pub(crate) fn destroy(&self) {
        unsafe {
            let _: () = msg_send![*self.layer, setContents: nil];
        }
    }

    fn rasterize(&self, scene: &Scene) -> Option<Canvas> {
        Self::rasterize_scene(scene, self.size, self.opaque, &self.atlas)
    }

    fn rasterize_scene(
        scene: &Scene,
        size: Size<DevicePixels>,
        opaque: bool,
        atlas: &SoftwareAtlas,
    ) -> Option<Canvas> {
        let width = u32::try_from(size.width.0).ok()?;
        let height = u32::try_from(size.height.0).ok()?;
        let mut canvas = Canvas::new(width, height, opaque)?;
        let atlas = atlas.0.lock();
        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Quads(range) => {
                    for quad in &scene.quads[range] {
                        canvas.quad(quad);
                    }
                }
                PrimitiveBatch::Shadows(range) => {
                    for shadow in &scene.shadows[range] {
                        canvas.shadow(shadow);
                    }
                }
                PrimitiveBatch::Underlines(range) => {
                    for underline in &scene.underlines[range] {
                        canvas.underline(underline);
                    }
                }
                PrimitiveBatch::Paths(range) => {
                    for path in &scene.paths[range] {
                        canvas.path(path);
                    }
                }
                PrimitiveBatch::MonochromeSprites { range, .. } => {
                    for sprite in &scene.monochrome_sprites[range] {
                        if let Some(tile) = atlas.backend.tiles.get(&sprite.tile.texture_id.index) {
                            canvas.sprite(
                                tile,
                                sprite.bounds,
                                sprite.content_mask,
                                Some(rgba(sprite.color)),
                                sprite.transformation,
                                1.,
                                false,
                                [0.; 4],
                            );
                        }
                    }
                }
                PrimitiveBatch::SubpixelSprites { range, .. } => {
                    for sprite in &scene.subpixel_sprites[range] {
                        if let Some(tile) = atlas.backend.tiles.get(&sprite.tile.texture_id.index) {
                            canvas.sprite(
                                tile,
                                sprite.bounds,
                                sprite.content_mask,
                                Some(rgba(sprite.color)),
                                sprite.transformation,
                                1.,
                                false,
                                [0.; 4],
                            );
                        }
                    }
                }
                PrimitiveBatch::PolychromeSprites { range, .. } => {
                    for sprite in &scene.polychrome_sprites[range] {
                        if let Some(tile) = atlas.backend.tiles.get(&sprite.tile.texture_id.index) {
                            canvas.sprite(
                                tile,
                                sprite.bounds,
                                sprite.content_mask,
                                None,
                                TransformationMatrix::unit(),
                                sprite.opacity,
                                sprite.grayscale != gpui::PaddedBool32::default(),
                                radii(sprite.corner_radii),
                            );
                        }
                    }
                }
                // Qrow has no video/camera surfaces. These require pixel-buffer conversion.
                PrimitiveBatch::Surfaces(_) => {}
            }
        }
        Some(canvas)
    }

    pub(crate) fn draw(&mut self, scene: &Scene) {
        let started = self.metrics.then(std::time::Instant::now);
        let Some(canvas) = self.rasterize(scene) else {
            return;
        };
        let width = canvas.width as usize;
        let height = canvas.height as usize;
        let provider = CGDataProvider::from_buffer(Arc::new(canvas.pixels));
        // RGBA bytes, premultiplied alpha, big-endian component ordering.
        let image = CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            &CGColorSpace::create_device_rgb(),
            1 | (4 << 12),
            &provider,
            false,
            0,
        );
        unsafe {
            let _: () = msg_send![*self.layer, setContents: image.as_ptr()];
        }
        if let Some(started) = started {
            eprintln!(
                "QROW_RENDER_FRAME backend=software time={:.6} ms={:.3}",
                crate::renderer::metric_time(),
                started.elapsed().as_secs_f64() * 1000.
            );
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn render_to_image(&mut self, scene: &Scene) -> Result<image::RgbaImage> {
        let canvas = self
            .rasterize(scene)
            .ok_or_else(|| anyhow!("empty software viewport"))?;
        Self::canvas_to_image(canvas)
    }

    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    fn canvas_to_image(mut canvas: Canvas) -> Result<image::RgbaImage> {
        // Image buffers expose straight alpha; the retained layer uses premultiplied alpha.
        for pixel in canvas.pixels.chunks_exact_mut(4) {
            if pixel[3] != 0 {
                for channel in 0..3 {
                    pixel[channel] =
                        ((pixel[channel] as u32 * 255) / pixel[3] as u32).min(255) as u8;
                }
            }
        }
        image::RgbaImage::from_raw(canvas.width, canvas.height, canvas.pixels)
            .ok_or_else(|| anyhow!("invalid software frame"))
    }
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
#[derive(Default)]
pub(crate) struct SoftwareHeadlessRenderer {
    atlas: Arc<SoftwareAtlas>,
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl gpui::PlatformHeadlessRenderer for SoftwareHeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<image::RgbaImage> {
        let canvas = SoftwareRenderer::rasterize_scene(scene, size, false, &self.atlas)
            .ok_or_else(|| anyhow!("empty software viewport"))?;
        SoftwareRenderer::canvas_to_image(canvas)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        SoftwareRenderer::rasterize_scene(scene, size, false, &self.atlas)
            .ok_or_else(|| anyhow!("empty software viewport"))?;
        Ok(())
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
}

fn rgba(color: Hsla) -> [f32; 4] {
    let c = Rgba::from(color);
    [c.r, c.g, c.b, c.a]
}
fn rect(bounds: Bounds<ScaledPixels>) -> [f32; 4] {
    [
        bounds.origin.x.0,
        bounds.origin.y.0,
        bounds.size.width.0,
        bounds.size.height.0,
    ]
}
fn radii(c: Corners<ScaledPixels>) -> [f32; 4] {
    [
        c.top_left.0,
        c.top_right.0,
        c.bottom_right.0,
        c.bottom_left.0,
    ]
}

// Signed distance to a rectangle with independently rounded corners.
fn distance(x: f32, y: f32, bounds: [f32; 4], corners: [f32; 4]) -> f32 {
    let [left, top, width, height] = bounds;
    let cx = x - left - width * 0.5;
    let cy = y - top - height * 0.5;
    let corner = match (cx > 0., cy > 0.) {
        (false, false) => 0,
        (true, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    };
    let r = corners[corner].max(0.).min(width.min(height).max(0.) * 0.5);
    let dx = cx.abs() - width * 0.5 + r;
    let dy = cy.abs() - height * 0.5 + r;
    dx.max(0.).hypot(dy.max(0.)) + dx.max(dy).min(0.) - r
}

enum Fill {
    Solid([f32; 4]),
    Gradient {
        angle: f32,
        colors: [[f32; 4]; 2],
        stops: [f32; 2],
    },
    Pattern {
        color: [f32; 4],
        size: f32,
        checkerboard: bool,
    },
}

impl Fill {
    fn new(background: Background) -> Self {
        if let Some(color) = background.as_solid() {
            return Self::Solid(rgba(color));
        }
        // Background exposes solid colors, but its non-solid fields are private.
        // Use its serialization contract instead of assuming its in-memory layout.
        let value = serde_json::to_value(background).expect("serialize GPUI background");
        let color = |value: &serde_json::Value| {
            rgba(serde_json::from_value(value.clone()).expect("GPUI background color"))
        };
        let dimension = value["gradient_angle_or_pattern_height"]
            .as_f64()
            .unwrap_or(0.) as f32;
        if value["tag"] == "LinearGradient" {
            Self::Gradient {
                angle: dimension,
                colors: [
                    color(&value["colors"][0]["color"]),
                    color(&value["colors"][1]["color"]),
                ],
                stops: [
                    value["colors"][0]["percentage"].as_f64().unwrap_or(0.) as f32,
                    value["colors"][1]["percentage"].as_f64().unwrap_or(1.) as f32,
                ],
            }
        } else {
            Self::Pattern {
                color: color(&value["solid"]),
                size: dimension,
                checkerboard: value["tag"] == "Checkerboard",
            }
        }
    }

    fn at(&self, x: f32, y: f32, bounds: [f32; 4]) -> [f32; 4] {
        match self {
            Self::Solid(color) => *color,
            Self::Gradient {
                angle,
                colors,
                stops,
            } => {
                let [left, top, width, height] = bounds;
                let theta = (angle - 90.).to_radians();
                let (mut dx, mut dy) = (theta.cos(), theta.sin());
                if width > height {
                    dy *= height / width;
                } else {
                    dx *= width / height;
                }
                let t =
                    ((x - left - width * 0.5) * dx + (y - top - height * 0.5) * dy) / dx.hypot(dy);
                let t = if dx.abs() > dy.abs() {
                    t / width + 0.5
                } else {
                    t / height + 0.5
                };
                let span = stops[1] - stops[0];
                let t = if span.abs() < f32::EPSILON {
                    if t >= stops[1] { 1. } else { 0. }
                } else {
                    ((t - stops[0]) / span).clamp(0., 1.)
                };
                std::array::from_fn(|i| colors[0][i] + (colors[1][i] - colors[0][i]) * t)
            }
            Self::Pattern {
                color,
                size,
                checkerboard,
            } => {
                let (x, y) = (x - bounds[0], y - bounds[1]);
                let alpha = if *checkerboard {
                    if ((x / size.max(1.)).floor() as i32 + (y / size.max(1.)).floor() as i32) % 2
                        != 0
                    {
                        1.
                    } else {
                        0.
                    }
                } else {
                    let width = (size / 65535.) / 255.;
                    let interval = size.rem_euclid(65535.) / 255.;
                    let period = (width + interval) * std::f32::consts::FRAC_1_SQRT_2;
                    if period <= 0. {
                        0.
                    } else {
                        let phase = ((x + y) * std::f32::consts::FRAC_1_SQRT_2).rem_euclid(period);
                        let d = phase.min(period - phase)
                            - width * std::f32::consts::FRAC_1_SQRT_2 * 0.5;
                        (0.5 - d).clamp(0., 1.)
                    }
                };
                let mut result = *color;
                result[3] *= alpha;
                result
            }
        }
    }
}

struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32, opaque: bool) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let length = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        // Bound allocation during malformed or transient native resizes.
        if length > 256 * 1024 * 1024 {
            return None;
        }
        let mut pixels = vec![0; length];
        if opaque {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    fn blend(&mut self, x: u32, y: u32, color: [f32; 4], coverage: f32) {
        let alpha = (color[3] * coverage).clamp(0., 1.);
        if alpha == 0. {
            return;
        }
        let start = ((y * self.width + x) * 4) as usize;
        let pixel = &mut self.pixels[start..start + 4];
        for i in 0..3 {
            pixel[i] =
                (color[i].clamp(0., 1.) * alpha * 255. + pixel[i] as f32 * (1. - alpha) + 0.5)
                    .min(255.) as u8;
        }
        pixel[3] = (alpha * 255. + pixel[3] as f32 * (1. - alpha) + 0.5).min(255.) as u8;
    }

    fn area(&self, bounds: [f32; 4], mask: ContentMask<ScaledPixels>) -> [u32; 4] {
        let clip = rect(mask.bounds);
        let left = bounds[0]
            .max(clip[0])
            .floor()
            .max(0.)
            .min(self.width as f32) as u32;
        let top = bounds[1]
            .max(clip[1])
            .floor()
            .max(0.)
            .min(self.height as f32) as u32;
        let right = (bounds[0] + bounds[2])
            .min(clip[0] + clip[2])
            .ceil()
            .max(left as f32)
            .min(self.width as f32) as u32;
        let bottom = (bounds[1] + bounds[3])
            .min(clip[1] + clip[3])
            .ceil()
            .max(top as f32)
            .min(self.height as f32) as u32;
        [left, top, right, bottom]
    }

    fn clip_coverage(x: u32, y: u32, mask: ContentMask<ScaledPixels>) -> f32 {
        let [left, top, width, height] = rect(mask.bounds);
        ((x as f32 + 1.).min(left + width) - (x as f32).max(left)).clamp(0., 1.)
            * ((y as f32 + 1.).min(top + height) - (y as f32).max(top)).clamp(0., 1.)
    }

    fn quad(&mut self, quad: &gpui::Quad) {
        let bounds = rect(quad.bounds);
        let corners = radii(quad.corner_radii);
        let [left, top, right, bottom] = self.area(bounds, quad.content_mask);
        let fill = Fill::new(quad.background);
        let border = rgba(quad.border_color);
        let widths = [
            quad.border_widths.top.0,
            quad.border_widths.right.0,
            quad.border_widths.bottom.0,
            quad.border_widths.left.0,
        ];
        let inner = [
            bounds[0] + widths[3],
            bounds[1] + widths[0],
            bounds[2] - widths[3] - widths[1],
            bounds[3] - widths[0] - widths[2],
        ];
        let inner_corners = [
            corners[0] - widths[0].max(widths[3]),
            corners[1] - widths[0].max(widths[1]),
            corners[2] - widths[2].max(widths[1]),
            corners[3] - widths[2].max(widths[3]),
        ];
        let has_border = widths.iter().any(|w| *w > 0.) && border[3] > 0.;
        for y in top..bottom {
            for x in left..right {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let coverage = (0.5 - distance(px, py, bounds, corners)).clamp(0., 1.);
                let clip = Self::clip_coverage(x, y, quad.content_mask);
                let mut color = fill.at(px, py, bounds);
                if has_border {
                    let inside = if inner[2] > 0. && inner[3] > 0. {
                        (0.5 - distance(px, py, inner, inner_corners)).clamp(0., 1.)
                    } else {
                        0.
                    };
                    let dashed = quad.border_style == BorderStyle::Dashed;
                    let position = if py < inner[1] || py > inner[1] + inner[3] {
                        px
                    } else {
                        py
                    };
                    let dash = widths.iter().copied().fold(1., f32::max) * 3.;
                    if !dashed || position.rem_euclid(dash * 2.) < dash {
                        let fraction = (coverage - inside).max(0.) / coverage.max(f32::EPSILON);
                        let alpha = border[3] + color[3] * (1. - border[3]);
                        if alpha > 0. {
                            let over: [f32; 4] = std::array::from_fn(|i| {
                                if i == 3 {
                                    alpha
                                } else {
                                    (border[i] * border[3] + color[i] * color[3] * (1. - border[3]))
                                        / alpha
                                }
                            });
                            color =
                                std::array::from_fn(|i| color[i] + (over[i] - color[i]) * fraction);
                        }
                    }
                }
                self.blend(x, y, color, coverage * clip);
            }
        }
    }

    fn shadow(&mut self, shadow: &gpui::Shadow) {
        let bounds = rect(shadow.bounds);
        let corners = radii(shadow.corner_radii);
        let element = rect(shadow.element_bounds);
        let element_corners = radii(shadow.element_corner_radii);
        let sigma = (shadow.blur_radius.0 * 0.5).max(0.5);
        let expanded = [
            bounds[0] - sigma * 3.,
            bounds[1] - sigma * 3.,
            bounds[2] + sigma * 6.,
            bounds[3] + sigma * 6.,
        ];
        let [left, top, right, bottom] = self.area(expanded, shadow.content_mask);
        let color = rgba(shadow.color);
        for y in top..bottom {
            for x in left..right {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = distance(px, py, bounds, corners);
                let edge = distance(px, py, element, element_corners);
                let coverage = if shadow.inset == 0 {
                    (0.5 + edge).clamp(0., 1.) / (1. + (d / sigma * 1.7).exp())
                } else {
                    (0.5 - edge).clamp(0., 1.) / (1. + (-d / sigma * 1.7).exp())
                };
                self.blend(
                    x,
                    y,
                    color,
                    coverage * Self::clip_coverage(x, y, shadow.content_mask),
                );
            }
        }
    }

    fn underline(&mut self, underline: &gpui::Underline) {
        let bounds = rect(underline.bounds);
        let [left, top, right, bottom] = self.area(bounds, underline.content_mask);
        let color = rgba(underline.color);
        let thickness = underline.thickness.0.max(0.1);
        for y in top..bottom {
            for x in left..right {
                let center = if underline.wavy != gpui::PaddedBool32::default() {
                    bounds[1]
                        + bounds[3] * 0.5
                        + ((x as f32 - bounds[0]) / thickness / 2.).sin() * thickness
                } else {
                    bounds[1] + thickness * 0.5
                };
                let coverage =
                    (thickness * 0.5 + 0.5 - (y as f32 + 0.5 - center).abs()).clamp(0., 1.);
                self.blend(
                    x,
                    y,
                    color,
                    coverage * Self::clip_coverage(x, y, underline.content_mask),
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sprite(
        &mut self,
        tile: &TilePixels,
        bounds: Bounds<ScaledPixels>,
        mask: ContentMask<ScaledPixels>,
        color: Option<[f32; 4]>,
        transform: TransformationMatrix,
        opacity: f32,
        grayscale: bool,
        corners: [f32; 4],
    ) {
        let bounds = rect(bounds);
        if bounds[2] <= 0. || bounds[3] <= 0. {
            return;
        }
        let m = transform.rotation_scale;
        let determinant = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        if determinant.abs() < f32::EPSILON {
            return;
        }
        let transformed = |x: f32, y: f32| {
            [
                m[0][0] * x + m[0][1] * y + transform.translation[0],
                m[1][0] * x + m[1][1] * y + transform.translation[1],
            ]
        };
        let points = [
            transformed(bounds[0], bounds[1]),
            transformed(bounds[0] + bounds[2], bounds[1]),
            transformed(bounds[0], bounds[1] + bounds[3]),
            transformed(bounds[0] + bounds[2], bounds[1] + bounds[3]),
        ];
        let left = points.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        let top = points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let right = points
            .iter()
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let bottom = points
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max);
        let [left, top, right, bottom] = self.area([left, top, right - left, bottom - top], mask);
        for y in top..bottom {
            for x in left..right {
                let dx = x as f32 + 0.5 - transform.translation[0];
                let dy = y as f32 + 0.5 - transform.translation[1];
                let px = (m[1][1] * dx - m[0][1] * dy) / determinant;
                let py = (-m[1][0] * dx + m[0][0] * dy) / determinant;
                let u = (px - bounds[0]) / bounds[2];
                let v = (py - bounds[1]) / bounds[3];
                if !(0. ..1.).contains(&u) || !(0. ..1.).contains(&v) {
                    continue;
                }
                let sample = tile.sample(u * tile.width as f32 - 0.5, v * tile.height as f32 - 0.5);
                let mut output = if let Some(mut color) = color {
                    color[3] *= sample[3];
                    color
                } else {
                    sample
                };
                if grayscale {
                    let l = output[0] * 0.2126 + output[1] * 0.7152 + output[2] * 0.0722;
                    output[0] = l;
                    output[1] = l;
                    output[2] = l;
                }
                output[3] *= opacity;
                let coverage = (0.5 - distance(px, py, bounds, corners)).clamp(0., 1.);
                self.blend(x, y, output, coverage * Self::clip_coverage(x, y, mask));
            }
        }
    }

    fn path(&mut self, path: &gpui::Path<ScaledPixels>) {
        let bounds = rect(path.bounds);
        let [left, top, right, bottom] = self.area(bounds, path.content_mask);
        if right == left || bottom == top {
            return;
        }
        let width = (right - left) as usize;
        let mut coverage = vec![0u8; width * (bottom - top) as usize];
        for vertices in path.vertices.chunks_exact(3) {
            let p: [[f32; 2]; 3] =
                std::array::from_fn(|i| [vertices[i].xy_position.x.0, vertices[i].xy_position.y.0]);
            let denominator = (p[1][1] - p[2][1]) * (p[0][0] - p[2][0])
                + (p[2][0] - p[1][0]) * (p[0][1] - p[2][1]);
            if denominator.abs() < f32::EPSILON {
                continue;
            }
            let x0 = p
                .iter()
                .map(|p| p[0])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(left as f32) as u32;
            let y0 = p
                .iter()
                .map(|p| p[1])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(top as f32) as u32;
            let x1 = p
                .iter()
                .map(|p| p[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min(right as f32)
                .max(x0 as f32) as u32;
            let y1 = p
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min(bottom as f32)
                .max(y0 as f32) as u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let mut bits = 0;
                    for (i, (ox, oy)) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)]
                        .iter()
                        .enumerate()
                    {
                        let px = x as f32 + ox;
                        let py = y as f32 + oy;
                        let a = ((p[1][1] - p[2][1]) * (px - p[2][0])
                            + (p[2][0] - p[1][0]) * (py - p[2][1]))
                            / denominator;
                        let b = ((p[2][1] - p[0][1]) * (px - p[2][0])
                            + (p[0][0] - p[2][0]) * (py - p[2][1]))
                            / denominator;
                        let c = 1. - a - b;
                        if a >= 0. && b >= 0. && c >= 0. {
                            let s = a * vertices[0].st_position.x
                                + b * vertices[1].st_position.x
                                + c * vertices[2].st_position.x;
                            let t = a * vertices[0].st_position.y
                                + b * vertices[1].st_position.y
                                + c * vertices[2].st_position.y;
                            if s * s <= t {
                                bits |= 1 << i;
                            }
                        }
                    }
                    coverage[(y - top) as usize * width + (x - left) as usize] |= bits;
                }
            }
        }
        let fill = Fill::new(path.color);
        for y in top..bottom {
            for x in left..right {
                let alpha = coverage[(y - top) as usize * width + (x - left) as usize].count_ones()
                    as f32
                    / 4.;
                self.blend(
                    x,
                    y,
                    fill.at(x as f32 + 0.5, y as f32 + 0.5, bounds),
                    alpha * Self::clip_coverage(x, y, path.content_mask),
                );
            }
        }
    }
}

impl TilePixels {
    fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        let offset = y * self.width + x;
        match self.kind {
            AtlasTextureKind::Monochrome => [1., 1., 1., self.bytes[offset] as f32 / 255.],
            // GPUI's Core Text and decoded-image atlases provide BGRA bytes.
            AtlasTextureKind::Polychrome => {
                let b = &self.bytes[offset * 4..offset * 4 + 4];
                [
                    b[2] as f32 / 255.,
                    b[1] as f32 / 255.,
                    b[0] as f32 / 255.,
                    b[3] as f32 / 255.,
                ]
            }
            AtlasTextureKind::Subpixel => {
                let b = &self.bytes[offset * 4..offset * 4 + 4];
                [1., 1., 1., (b[0] as f32 + b[1] as f32 + b[2] as f32) / 765.]
            }
        }
    }

    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let sample = |x: f32, y: f32| {
            let mut pixel = self.pixel(
                x.clamp(0., self.width as f32 - 1.) as usize,
                y.clamp(0., self.height as f32 - 1.) as usize,
            );
            if self.kind == AtlasTextureKind::Polychrome {
                // Filter premultiplied colors so transparent texels do not
                // darken edges or contribute their hidden RGB values.
                for channel in 0..3 {
                    pixel[channel] *= pixel[3];
                }
            }
            pixel
        };
        let fx = x - x.floor();
        let fy = y - y.floor();
        let pixels = [
            sample(x.floor(), y.floor()),
            sample(x.floor() + 1., y.floor()),
            sample(x.floor(), y.floor() + 1.),
            sample(x.floor() + 1., y.floor() + 1.),
        ];
        let mut output = std::array::from_fn(|i| {
            (pixels[0][i] * (1. - fx) + pixels[1][i] * fx) * (1. - fy)
                + (pixels[2][i] * (1. - fx) + pixels[3][i] * fx) * fy
        });
        if self.kind == AtlasTextureKind::Polychrome && output[3] > 0. {
            // Canvas::blend receives straight colors and applies coverage.
            for channel in 0..3 {
                output[channel] /= output[3];
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(y)),
            size: size(ScaledPixels(w), ScaledPixels(h)),
        }
    }
    fn mask(x: f32, y: f32, w: f32, h: f32) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: bounds(x, y, w, h),
        }
    }
    fn pixel(canvas: &Canvas, x: u32, y: u32) -> &[u8] {
        let offset = ((y * canvas.width + x) * 4) as usize;
        &canvas.pixels[offset..offset + 4]
    }

    #[test]
    fn clipped_translucent_quad_preserves_premultiplied_alpha() {
        let mut canvas = Canvas::new(4, 4, false).unwrap();
        canvas.quad(&gpui::Quad {
            bounds: bounds(0., 0., 4., 4.),
            content_mask: mask(1.5, 1., 2., 2.),
            background: gpui::rgba(0xff000080).into(),
            ..Default::default()
        });
        assert_eq!(pixel(&canvas, 0, 1), &[0, 0, 0, 0]);
        assert_eq!(pixel(&canvas, 1, 1), &[64, 0, 0, 64]);
        assert_eq!(pixel(&canvas, 2, 1), &[128, 0, 0, 128]);
        assert_eq!(pixel(&canvas, 2, 3), &[0, 0, 0, 0]);
        canvas.blend(2, 1, [0., 0., 1., 0.5], 1.);
        assert_eq!(pixel(&canvas, 2, 1), &[64, 0, 128, 192]);
    }

    #[test]
    fn unequal_borders_and_rounded_corners_keep_the_fill() {
        let mut canvas = Canvas::new(10, 10, false).unwrap();
        canvas.quad(&gpui::Quad {
            bounds: bounds(0., 0., 10., 10.),
            content_mask: mask(0., 0., 10., 10.),
            background: gpui::rgba(0x00ff00ff).into(),
            border_color: gpui::rgba(0xff0000ff).into(),
            corner_radii: gpui::Corners::all(ScaledPixels(3.)),
            border_widths: gpui::Edges {
                top: ScaledPixels(2.),
                right: ScaledPixels(1.),
                bottom: ScaledPixels(1.),
                left: ScaledPixels(1.),
            },
            ..Default::default()
        });
        assert_eq!(pixel(&canvas, 0, 0), &[0, 0, 0, 0]);
        assert_eq!(pixel(&canvas, 5, 1), &[255, 0, 0, 255]);
        assert_eq!(pixel(&canvas, 5, 3), &[0, 255, 0, 255]);
        let coverage = (0.5 - distance(0.5, 1.5, [0., 0., 10., 10.], [3.; 4])).clamp(0., 1.);
        assert_eq!(
            pixel(&canvas, 0, 1)[3],
            (coverage * 255. + 0.5) as u8,
            "background and border must share one outer-edge coverage"
        );
    }

    #[test]
    fn patterns_keep_their_phase_when_the_element_moves() {
        for background in [
            gpui::checkerboard(gpui::rgba(0xff0000ff), 2.),
            gpui::pattern_slash(gpui::rgba(0xff0000ff), 1., 3.),
        ] {
            let fill = Fill::new(background);
            assert_eq!(
                fill.at(2.5, 0.5, [0., 0., 8., 8.]),
                fill.at(12.5, 20.5, [10., 20., 8., 8.])
            );
        }
        let checker = Fill::new(gpui::checkerboard(gpui::rgba(0xff0000ff), 2.));
        assert_eq!(checker.at(0.5, 0.5, [0., 0., 8., 8.])[3], 0.);
        assert_eq!(checker.at(2.5, 0.5, [0., 0., 8., 8.])[3], 1.);
    }

    #[test]
    fn atlas_checks_storage_and_releases_removed_tiles() {
        let mut atlas = AtlasPixels::default();
        let dimensions = size(DevicePixels(2), DevicePixels(1));
        assert!(
            atlas
                .insert(AtlasTextureKind::Monochrome, dimensions, &[255])
                .is_err()
        );
        let a = atlas
            .insert(AtlasTextureKind::Monochrome, dimensions, &[255, 0])
            .unwrap();
        let b = atlas
            .insert(AtlasTextureKind::Monochrome, dimensions, &[0, 255])
            .unwrap();
        assert_ne!(a.texture_id, b.texture_id);
        atlas.remove(a);
        assert!(!atlas.tiles.contains_key(&a.texture_id.index));
        assert_eq!(atlas.tiles[&b.texture_id.index].bytes, [0, 255]);
    }

    #[test]
    fn glyph_color_transforms_and_bgra_images_are_composited() {
        let mut canvas = Canvas::new(5, 3, false).unwrap();
        let glyph = TilePixels {
            width: 2,
            height: 1,
            kind: AtlasTextureKind::Monochrome,
            bytes: vec![255, 0],
        };
        let transform = TransformationMatrix {
            translation: [1., 1.],
            ..TransformationMatrix::unit()
        };
        canvas.sprite(
            &glyph,
            bounds(0., 0., 2., 1.),
            mask(0., 0., 5., 3.),
            Some([0., 1., 0., 1.]),
            transform,
            1.,
            false,
            [0.; 4],
        );
        assert_eq!(pixel(&canvas, 1, 1), &[0, 255, 0, 255]);
        assert_eq!(pixel(&canvas, 2, 1), &[0, 0, 0, 0]);
        let image = TilePixels {
            width: 1,
            height: 1,
            kind: AtlasTextureKind::Polychrome,
            bytes: vec![0, 0, 255, 255],
        };
        canvas.sprite(
            &image,
            bounds(3., 1., 1., 1.),
            mask(0., 0., 5., 3.),
            None,
            TransformationMatrix::unit(),
            0.5,
            false,
            [0.; 4],
        );
        assert_eq!(pixel(&canvas, 3, 1), &[128, 0, 0, 128]);
    }

    #[test]
    fn transparent_image_filtering_keeps_edge_colors() {
        for transparent in [[0, 0, 0, 0], [0, 255, 0, 0]] {
            let image = TilePixels {
                width: 2,
                height: 1,
                kind: AtlasTextureKind::Polychrome,
                bytes: [vec![0, 0, 255, 255], transparent.to_vec()].concat(),
            };
            assert_eq!(image.sample(0.5, 0.), [1., 0., 0., 0.5]);
            assert_eq!(image.sample(1., 0.), [0.; 4]);
            let mut canvas = Canvas::new(3, 1, false).unwrap();
            canvas.sprite(
                &image,
                bounds(0., 0., 3., 1.),
                mask(0., 0., 3., 1.),
                None,
                TransformationMatrix::unit(),
                1.,
                false,
                [0.; 4],
            );
            assert_eq!(pixel(&canvas, 0, 0), &[255, 0, 0, 255]);
            assert_eq!(pixel(&canvas, 1, 0), &[128, 0, 0, 128]);
            assert_eq!(pixel(&canvas, 2, 0), &[0, 0, 0, 0]);
        }
    }

    #[test]
    fn adjacent_path_triangles_do_not_double_blend_their_shared_edge() {
        let mut canvas = Canvas::new(4, 4, false).unwrap();
        let mut path = gpui::Path::new(point(gpui::px(0.), gpui::px(0.)));
        path.line_to(point(gpui::px(4.), gpui::px(0.)));
        path.line_to(point(gpui::px(4.), gpui::px(4.)));
        path.line_to(point(gpui::px(0.), gpui::px(4.)));
        let mut path = path.scale(1.);
        path.color = gpui::rgba(0xff000080).into();
        path.content_mask = mask(0., 0., 4., 4.);
        canvas.path(&path);
        assert!(canvas.pixels.chunks_exact(4).all(|p| p == [128, 0, 0, 128]));
    }

    #[test]
    fn empty_or_excessive_viewports_do_not_allocate() {
        assert!(Canvas::new(0, 1, true).is_none());
        assert!(Canvas::new(u32::MAX, u32::MAX, true).is_none());
    }

    #[test]
    fn headless_scene_uses_the_same_compositor_and_straight_alpha() {
        use gpui::PlatformHeadlessRenderer;
        let mut scene = Scene::default();
        scene.insert_primitive(gpui::Quad {
            bounds: bounds(0., 0., 2., 2.),
            content_mask: mask(0., 0., 2., 2.),
            background: gpui::rgba(0xff000080).into(),
            ..Default::default()
        });
        scene.finish();
        let mut renderer = SoftwareHeadlessRenderer::default();
        let dimensions = size(DevicePixels(2), DevicePixels(2));
        renderer.render_scene(&scene, dimensions).unwrap();
        let image = renderer.render_scene_to_image(&scene, dimensions).unwrap();
        assert_eq!(image.dimensions(), (2, 2));
        assert!(image.pixels().all(|p| p.0 == [255, 0, 0, 128]));
        assert!(
            renderer
                .render_scene(&scene, size(DevicePixels(0), DevicePixels(2)))
                .is_err()
        );
    }
}
