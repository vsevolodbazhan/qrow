//! Select a renderer without requiring a Metal device to create a window.
use crate::opengl_renderer::OpenGlRenderer;
use crate::software_renderer::SoftwareRenderer;
use gpui::{DevicePixels, PlatformAtlas, Scene, Size};
use gpui_apple::metal_renderer;
use objc::{msg_send, sel, sel_impl};
use std::{ffi::c_void, sync::Arc};

pub(crate) type Context = metal_renderer::Context;

/// DispatchTime in the native test driver uses this same Mach uptime clock.
pub(crate) fn metric_time() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let error = unsafe { libc::clock_gettime(libc::CLOCK_UPTIME_RAW, &mut time) };
    assert_eq!(error, 0, "Mach uptime clock");
    time.tv_sec as f64 + time.tv_nsec as f64 / 1_000_000_000.
}

pub(crate) struct Renderer {
    backend: Backend,
    atlas: Arc<dyn PlatformAtlas>,
}

enum Backend {
    Metal(metal_renderer::Renderer),
    Software(SoftwareRenderer),
    OpenGl(OpenGlRenderer),
}

pub(crate) unsafe fn new_renderer(
    context: Context,
    native_window: *mut c_void,
    native_view: *mut c_void,
    bounds: Size<f32>,
    transparent: bool,
) -> Renderer {
    let preference = preference_or_exit();
    let metal_available = metal::Device::system_default().is_some();
    if preference == Preference::Metal && !metal_available {
        eprintln!("Qrow: Metal initialization failed: no Metal device is available");
        std::process::exit(1);
    }
    let backend = if preference == Preference::OpenGl
        || (preference == Preference::Auto && !metal_available)
    {
        let scale: f64 = unsafe { msg_send![native_window as cocoa::base::id, backingScaleFactor] };
        match OpenGlRenderer::new(bounds, scale as f32, transparent) {
            Ok(renderer) => Backend::OpenGl(renderer),
            Err(error) if preference == Preference::Auto => {
                eprintln!(
                    "Qrow: OpenGL initialization failed; using CPU rendering: {error:#}"
                );
                Backend::Software(unsafe {
                    SoftwareRenderer::new(bounds, scale as f32, transparent)
                })
            }
            Err(error) => {
                eprintln!("Qrow: OpenGL initialization failed: {error:#}");
                std::process::exit(1)
            }
        }
    } else if preference == Preference::Software {
        eprintln!("Qrow: using CPU rendering (software rendering is requested)");
        let scale: f64 = unsafe { msg_send![native_window as cocoa::base::id, backingScaleFactor] };
        Backend::Software(unsafe { SoftwareRenderer::new(bounds, scale as f32, transparent) })
    } else if metal_available {
        Backend::Metal(unsafe {
            metal_renderer::new_renderer(context, native_window, native_view, bounds, transparent)
        })
    } else {
        unreachable!("auto selects OpenGL and explicit Metal is checked above")
    };
    let atlas: Arc<dyn PlatformAtlas> = match &backend {
        Backend::Metal(renderer) => renderer.sprite_atlas().clone(),
        Backend::Software(renderer) => renderer.sprite_atlas().clone(),
        Backend::OpenGl(renderer) => renderer.sprite_atlas().clone(),
    };
    Renderer { backend, atlas }
}

impl Renderer {
    pub(crate) fn layer_ptr(&self) -> *mut c_void {
        match &self.backend {
            Backend::Metal(renderer) => renderer.layer_ptr().cast(),
            Backend::Software(renderer) => renderer.layer_ptr(),
            Backend::OpenGl(renderer) => renderer.layer_ptr(),
        }
    }

    pub(crate) fn sprite_atlas(&self) -> &Arc<dyn PlatformAtlas> {
        &self.atlas
    }

    pub(crate) fn update_drawable_size(&mut self, size: Size<DevicePixels>) {
        match &mut self.backend {
            Backend::Metal(renderer) => renderer.update_drawable_size(size),
            Backend::Software(renderer) => renderer.update_drawable_size(size),
            Backend::OpenGl(renderer) => renderer.update_drawable_size(size),
        }
    }

    pub(crate) fn update_transparency(&mut self, transparent: bool) {
        match &mut self.backend {
            Backend::Metal(renderer) => renderer.update_transparency(transparent),
            Backend::Software(renderer) => renderer.update_transparency(transparent),
            Backend::OpenGl(renderer) => renderer.update_transparency(transparent),
        }
    }

    pub(crate) fn set_presents_with_transaction(&mut self, value: bool) {
        if let Backend::Metal(renderer) = &mut self.backend {
            renderer.set_presents_with_transaction(value);
        }
    }

    pub(crate) fn destroy(&self) {
        match &self.backend {
            Backend::Metal(renderer) => renderer.destroy(),
            Backend::Software(renderer) => renderer.destroy(),
            Backend::OpenGl(renderer) => renderer.destroy(),
        }
    }

    pub(crate) fn draw(&mut self, scene: &Scene) {
        match &mut self.backend {
            Backend::Metal(renderer) => renderer.draw(scene),
            Backend::Software(renderer) => renderer.draw(scene),
            Backend::OpenGl(renderer) => renderer.draw(scene),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn render_to_image(&mut self, scene: &Scene) -> anyhow::Result<image::RgbaImage> {
        match &mut self.backend {
            Backend::Metal(renderer) => renderer.render_to_image(scene),
            Backend::Software(renderer) => renderer.render_to_image(scene),
            Backend::OpenGl(renderer) => renderer.render_to_image(scene),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Preference {
    Auto,
    Metal,
    OpenGl,
    Software,
}

fn parse_preference(
    value: Option<&std::ffi::OsStr>,
    legacy: Option<&std::ffi::OsStr>,
) -> anyhow::Result<Preference> {
    use std::ffi::OsStr;
    Ok(match value {
        Some(value) if value == OsStr::new("auto") => Preference::Auto,
        Some(value) if value == OsStr::new("metal") => Preference::Metal,
        Some(value) if value == OsStr::new("opengl") => Preference::OpenGl,
        Some(value) if value == OsStr::new("software") => Preference::Software,
        Some(_) => anyhow::bail!("QROW_RENDERER must be auto, metal, opengl, or software"),
        None if legacy == Some(OsStr::new("1")) => Preference::Software,
        None => Preference::Auto,
    })
}

fn preference_or_exit() -> Preference {
    parse_preference(
        std::env::var_os("QROW_RENDERER").as_deref(),
        std::env::var_os("QROW_SOFTWARE_RENDERER").as_deref(),
    )
    .unwrap_or_else(|error| {
        eprintln!("Qrow: renderer selection failed: {error}");
        std::process::exit(1)
    })
}

/// The headless platform uses the same renderer selection as native windows.
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
pub struct HeadlessRenderer(Box<dyn gpui::PlatformHeadlessRenderer>);

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl Default for HeadlessRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl HeadlessRenderer {
    pub fn new() -> Self {
        let preference = preference_or_exit();
        let metal_available = metal::Device::system_default().is_some();
        if preference == Preference::Metal && !metal_available {
            eprintln!("Qrow: Metal initialization failed: no Metal device is available");
            std::process::exit(1);
        }
        if preference == Preference::OpenGl
            || (preference == Preference::Auto && !metal_available)
        {
            Self(Box::new(
                match crate::opengl_renderer::OpenGlHeadlessRenderer::new() {
                    Ok(renderer) => renderer,
                    Err(error) if preference == Preference::Auto => {
                        eprintln!(
                            "Qrow: OpenGL initialization failed; using CPU rendering: {error:#}"
                        );
                        return Self(Box::new(
                            crate::software_renderer::SoftwareHeadlessRenderer::default(),
                        ));
                    }
                    Err(error) => {
                        eprintln!("Qrow: OpenGL initialization failed: {error:#}");
                        std::process::exit(1)
                    }
                },
            ))
        } else if preference == Preference::Software {
            Self(Box::new(
                crate::software_renderer::SoftwareHeadlessRenderer::default(),
            ))
        } else if metal_available {
            Self(Box::new(metal_renderer::MetalHeadlessRenderer::new()))
        } else {
            unreachable!("auto selects OpenGL and explicit Metal is checked above")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    #[test]
    fn renderer_preference_is_validated_and_explicit_values_override_the_alias() {
        let parse = |value: Option<&str>, legacy: Option<&str>| {
            parse_preference(value.map(OsStr::new), legacy.map(OsStr::new))
        };
        assert_eq!(parse(None, None).unwrap(), Preference::Auto);
        assert_eq!(parse(None, Some("1")).unwrap(), Preference::Software);
        assert_eq!(parse(None, Some("0")).unwrap(), Preference::Auto);
        for (value, expected) in [
            ("auto", Preference::Auto),
            ("metal", Preference::Metal),
            ("opengl", Preference::OpenGl),
            ("software", Preference::Software),
        ] {
            assert_eq!(parse(Some(value), Some("1")).unwrap(), expected);
        }
        assert!(parse(Some("typo"), Some("1")).is_err());
        assert!(parse(Some(""), None).is_err());
    }
}

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
impl gpui::PlatformHeadlessRenderer for HeadlessRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        self.0.render_scene_to_image(scene, size)
    }
    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> anyhow::Result<()> {
        self.0.render_scene(scene, size)
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.sprite_atlas()
    }
}
