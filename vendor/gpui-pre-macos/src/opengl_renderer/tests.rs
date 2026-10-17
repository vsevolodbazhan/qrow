//! Actual GPU pixel tests. Contexts require a logged-in macOS session.
use super::*;
use gpui::{
    AtlasTextureKind, Bounds, ContentMask, PlatformHeadlessRenderer, Quad, ScaledPixels,
    TransformationMatrix, point, rgba, size,
};

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(w), ScaledPixels(h)),
    }
}
fn quad(b: Bounds<ScaledPixels>, c: u32) -> Quad {
    Quad {
        bounds: b,
        content_mask: ContentMask {
            bounds: bounds(0., 0., 16., 16.),
        },
        background: rgba(c).into(),
        ..Default::default()
    }
}
fn scene(quads: Vec<Quad>) -> Scene {
    let mut scene = Scene::default();
    for q in quads {
        scene.insert_primitive(q);
    }
    scene.finish();
    scene
}
fn image(scene: &Scene) -> image::RgbaImage {
    OpenGlHeadlessRenderer::new()
        .unwrap()
        .render_scene_to_image(scene, size(DevicePixels(16), DevicePixels(16)))
        .unwrap()
}
fn close(pixel: &image::Rgba<u8>, expected: [u8; 4]) {
    assert!(
        pixel
            .0
            .iter()
            .zip(expected)
            .all(|(&a, b)| (a as i16 - b as i16).abs() <= 2),
        "pixel {:?}, expected {expected:?}",
        pixel.0
    );
}

#[test]
fn hardware_shader_blending_and_fractional_clipping() {
    let mut red = quad(bounds(0., 0., 16., 16.), 0xff000080);
    red.content_mask = ContentMask {
        bounds: bounds(1.5, 1., 2., 2.),
    };
    let blue = quad(bounds(2., 1., 1., 1.), 0x0000ff80);
    let output = image(&scene(vec![red, blue]));
    close(output.get_pixel(0, 1), [0, 0, 0, 0]);
    close(output.get_pixel(1, 1), [255, 0, 0, 64]);
    close(output.get_pixel(2, 1), [85, 0, 170, 192]);
    close(output.get_pixel(2, 3), [0, 0, 0, 0]);
}

#[test]
fn borders_and_rounding_apply_outer_alpha_once() {
    let mut q = quad(bounds(0., 0., 10., 10.), 0x00ff00ff);
    q.corner_radii = gpui::Corners::all(ScaledPixels(3.));
    q.border_color = rgba(0xff0000ff).into();
    q.border_widths = gpui::Edges {
        top: ScaledPixels(2.),
        right: ScaledPixels(1.),
        bottom: ScaledPixels(1.),
        left: ScaledPixels(1.),
    };
    let output = image(&scene(vec![q]));
    close(output.get_pixel(0, 0), [0, 0, 0, 0]);
    close(output.get_pixel(5, 1), [255, 0, 0, 255]);
    close(output.get_pixel(5, 3), [0, 255, 0, 255]);
    assert!(output.get_pixel(0, 1)[3] > 0 && output.get_pixel(0, 1)[3] < 255);
}

#[test]
fn transformed_glyph_and_bgra_image_use_atlas_tiles() {
    let mut renderer = OpenGlHeadlessRenderer::new().unwrap();
    // Synthetic pixel data isolates sampling from installed fonts and images.
    let tile = renderer
        .atlas
        .test_insert(
            AtlasTextureKind::Monochrome,
            size(DevicePixels(2), DevicePixels(1)),
            &[255, 0],
        )
        .unwrap();
    let picture = renderer
        .atlas
        .test_insert(
            AtlasTextureKind::Polychrome,
            size(DevicePixels(1), DevicePixels(1)),
            &[0, 0, 255, 255],
        )
        .unwrap();
    let mut scene = Scene::default();
    scene.insert_primitive(gpui::MonochromeSprite {
        order: Default::default(),
        pad: 0,
        bounds: bounds(0., 0., 2., 1.),
        content_mask: ContentMask {
            bounds: bounds(0., 0., 16., 16.),
        },
        color: rgba(0x00ff00ff).into(),
        tile,
        transformation: TransformationMatrix {
            translation: [1., 1.],
            ..TransformationMatrix::unit()
        },
    });
    scene.insert_primitive(gpui::PolychromeSprite {
        order: Default::default(),
        pad: 0,
        grayscale: Default::default(),
        opacity: 0.5,
        bounds: bounds(3., 1., 1., 1.),
        content_mask: ContentMask {
            bounds: bounds(0., 0., 16., 16.),
        },
        corner_radii: Default::default(),
        tile: picture,
    });
    scene.finish();
    let output = renderer
        .render_scene_to_image(&scene, size(DevicePixels(16), DevicePixels(16)))
        .unwrap();
    close(output.get_pixel(1, 1), [0, 255, 0, 255]);
    close(output.get_pixel(2, 1), [0, 0, 0, 0]);
    close(output.get_pixel(3, 1), [255, 0, 0, 128]);
    // Repeated frames must reuse GL textures; no new upload is required.
    renderer
        .render_scene_to_image(&scene, size(DevicePixels(16), DevicePixels(16)))
        .unwrap();
    assert_eq!(renderer.engine.texture_count(), 2);
}

#[test]
fn adjacent_path_triangles_do_not_double_blend() {
    let mut p = gpui::Path::new(point(gpui::px(0.), gpui::px(0.)));
    for (x, y) in [(4., 0.), (4., 4.), (0., 4.)] {
        p.line_to(point(gpui::px(x), gpui::px(y)));
    }
    let mut p = p.scale(1.);
    p.color = rgba(0xff000080).into();
    p.content_mask = ContentMask {
        bounds: bounds(0., 0., 16., 16.),
    };
    let mut s = Scene::default();
    s.insert_primitive(p);
    s.finish();
    let output = image(&s);
    for y in 0..4 {
        for x in 0..4 {
            close(output.get_pixel(x, y), [255, 0, 0, 128]);
        }
    }
    close(output.get_pixel(4, 4), [0, 0, 0, 0]);
}

#[test]
fn curve_keeps_antialiasing_outside_its_mathematical_edge() {
    let mut renderer = OpenGlHeadlessRenderer::new().unwrap();
    let mut frame = scene::Frame::default();
    frame.triangle(
        scene::Vertex {
            mask: [0., 0., 16., 16.],
            color: [1., 0., 0., 1.],
            params: [3., 0., 0., 0.],
            ..Default::default()
        },
        [[0., 0., 0., 0.], [16., 0., 1., 0.], [0., 16., 0., 1.]],
    );
    let output = render_image(
        &mut renderer.engine,
        &frame,
        &renderer.atlas,
        size(DevicePixels(16), DevicePixels(16)),
    )
    .unwrap();
    let alpha = output.get_pixel(5, 1)[3];
    assert!(alpha > 0 && alpha < 128, "outside curve alpha: {alpha}");
    close(output.get_pixel(1, 5), [255, 0, 0, 255]);
    close(output.get_pixel(8, 1), [0, 0, 0, 0]);
}

#[test]
fn image_edges_remain_opaque_when_enlarged() {
    let mut renderer = OpenGlHeadlessRenderer::new().unwrap();
    let tile = renderer
        .atlas
        .test_insert(
            AtlasTextureKind::Polychrome,
            size(DevicePixels(1), DevicePixels(1)),
            &[0, 0, 255, 255],
        )
        .unwrap();
    let mut s = Scene::default();
    s.insert_primitive(gpui::PolychromeSprite {
        order: Default::default(),
        pad: 0,
        grayscale: Default::default(),
        opacity: 1.,
        bounds: bounds(1., 1., 4., 4.),
        content_mask: ContentMask {
            bounds: bounds(0., 0., 16., 16.),
        },
        corner_radii: Default::default(),
        tile,
    });
    s.finish();
    let output = renderer
        .render_scene_to_image(&s, size(DevicePixels(16), DevicePixels(16)))
        .unwrap();
    for y in 1..5 {
        for x in 1..5 {
            close(output.get_pixel(x, y), [255, 0, 0, 255]);
        }
    }
}

#[test]
fn frame_recovers_from_external_gl_and_unpack_state() {
    use glow::HasContext;
    let mut renderer = OpenGlHeadlessRenderer::new().unwrap();
    let before = unsafe { cgl::CGLGetCurrentContext() };
    renderer
        .engine
        .context
        .with_current(|| {
            unsafe {
                let gl = &renderer.engine.gl;
                gl.enable(glow::RASTERIZER_DISCARD);
                gl.enable(glow::FRAMEBUFFER_SRGB);
                gl.enable(glow::STENCIL_TEST);
                gl.enable(glow::COLOR_LOGIC_OP);
                gl.color_mask(false, false, false, false);
                gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 12);
                gl.pixel_store_i32(glow::UNPACK_SKIP_ROWS, 1);
                gl.pixel_store_i32(glow::UNPACK_SKIP_PIXELS, 1);
            }
            Ok(())
        })
        .unwrap();
    let tile = renderer
        .atlas
        .test_insert(
            AtlasTextureKind::Polychrome,
            size(DevicePixels(1), DevicePixels(1)),
            &[0, 0, 255, 255],
        )
        .unwrap();
    let mut s = Scene::default();
    s.insert_primitive(gpui::PolychromeSprite {
        order: Default::default(),
        pad: 0,
        grayscale: Default::default(),
        opacity: 1.,
        bounds: bounds(1., 1., 1., 1.),
        content_mask: ContentMask {
            bounds: bounds(0., 0., 16., 16.),
        },
        corner_radii: Default::default(),
        tile,
    });
    s.finish();
    let output = renderer
        .render_scene_to_image(&s, size(DevicePixels(16), DevicePixels(16)))
        .unwrap();
    close(output.get_pixel(1, 1), [255, 0, 0, 255]);
    assert_eq!(unsafe { cgl::CGLGetCurrentContext() }, before);
    renderer
        .engine
        .context
        .with_current(|| {
            unsafe {
                let gl = &renderer.engine.gl;
                for capability in [
                    glow::RASTERIZER_DISCARD,
                    glow::FRAMEBUFFER_SRGB,
                    glow::STENCIL_TEST,
                    glow::COLOR_LOGIC_OP,
                ] {
                    assert!(!gl.is_enabled(capability));
                }
                for parameter in [
                    glow::UNPACK_ROW_LENGTH,
                    glow::UNPACK_SKIP_ROWS,
                    glow::UNPACK_SKIP_PIXELS,
                ] {
                    assert_eq!(gl.get_parameter_i32(parameter), 0);
                }
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn srgb_gradients_and_patterns_match_reference_pixels() {
    for background in [
        gpui::linear_gradient(
            90.,
            gpui::linear_color_stop(rgba(0xff000080), 0.),
            gpui::linear_color_stop(rgba(0x0000ffff), 1.),
        )
        .color_space(gpui::ColorSpace::Srgb),
        gpui::linear_gradient(
            45.,
            gpui::linear_color_stop(rgba(0xff0000ff), 0.),
            gpui::linear_color_stop(rgba(0x00ff00ff), 1.),
        )
        .color_space(gpui::ColorSpace::Srgb),
        gpui::checkerboard(rgba(0xff0000ff), 2.),
        gpui::pattern_slash(rgba(0xff0000ff), 1., 3.),
    ] {
        let mut q = quad(bounds(2., 3., 12., 8.), 0);
        q.background = background;
        let s = scene(vec![q]);
        let gpu = image(&s);
        let cpu = crate::software_renderer::SoftwareHeadlessRenderer::default()
            .render_scene_to_image(&s, size(DevicePixels(16), DevicePixels(16)))
            .unwrap();
        for y in 0..16 {
            for x in 0..16 {
                close(gpu.get_pixel(x, y), cpu.get_pixel(x, y).0);
            }
        }
    }
}

#[test]
fn rectangles_upload_one_affine_instance_each() {
    use std::mem::size_of;
    let s = scene(vec![
        quad(bounds(0., 0., 8., 8.), 0xff000080),
        quad(bounds(8., 0., 8., 8.), 0x00ff0080),
    ]);
    let frame = scene::Frame::encode(&s);
    assert_eq!(frame.instances.len(), 2);
    assert_eq!(frame.batches.len(), 1);
    assert_eq!(frame.batches[0].vertices_per_instance, 6);
    assert_eq!(size_of::<scene::Instance>(), 224);
}

#[test]
fn rotated_sprites_and_mixed_batches_keep_positions_and_paint_order() {
    fn render() -> image::RgbaImage {
        let mut renderer = OpenGlHeadlessRenderer {
            engine: Engine::new().unwrap(),
            atlas: Arc::default(),
        };
        let tile = renderer
            .atlas
            .test_insert(
                AtlasTextureKind::Monochrome,
                size(DevicePixels(2), DevicePixels(2)),
                &[255, 0, 128, 255],
            )
            .unwrap();
        let mut s = Scene::default();
        s.insert_primitive(quad(bounds(0., 0., 16., 16.), 0x102030ff));
        let mut p = gpui::Path::new(point(gpui::px(1.), gpui::px(1.)));
        for (x, y) in [(10., 1.), (10., 10.), (1., 10.)] {
            p.line_to(point(gpui::px(x), gpui::px(y)));
        }
        let mut p = p.scale(1.);
        p.color = rgba(0xff000080).into();
        p.content_mask = ContentMask {
            bounds: bounds(0., 0., 16., 16.),
        };
        s.insert_primitive(p);
        for shift in [0., 4.] {
            s.insert_primitive(gpui::MonochromeSprite {
                order: Default::default(),
                pad: 0,
                bounds: bounds(1., 2., 4., 4.),
                content_mask: ContentMask {
                    bounds: bounds(0., 0., 16., 16.),
                },
                color: rgba(0x00ff00c0).into(),
                tile,
                transformation: TransformationMatrix {
                    rotation_scale: [[0., -1.], [1., 0.]],
                    translation: [10. + shift, 4.],
                },
            });
        }
        s.insert_primitive(quad(bounds(3., 3., 3., 3.), 0x0000ff80));
        s.finish();
        let frame = scene::Frame::encode(&s);
        assert!(frame.batches.len() >= 3);
        assert!(
            frame
                .batches
                .iter()
                .any(|b| b.vertices_per_instance == 3 && b.first > 0)
        );
        assert!(
            frame
                .batches
                .iter()
                .any(|b| b.vertices_per_instance == 6 && b.first > 0)
        );
        let image = render_image(
            &mut renderer.engine,
            &frame,
            &renderer.atlas,
            size(DevicePixels(16), DevicePixels(16)),
        )
        .unwrap();
        assert!(image.pixels().any(|p| p.0 != [16, 32, 48, 255]));
        image
    }
    let output = render();
    close(output.get_pixel(15, 15), [16, 32, 48, 255]);
    close(output.get_pixel(2, 2), [136, 16, 24, 255]);
    close(output.get_pixel(3, 3), [68, 8, 140, 255]);
    // Both rotated sprites occupy the expected translated bounds.
    assert!(output.get_pixel(7, 5)[1] > 32);
    assert!(output.get_pixel(11, 5)[1] > 32);
    close(output.get_pixel(3, 5), [68, 8, 140, 255]);
}

#[test]
fn context_requires_hardware_core_profile() {
    use glow::HasContext;
    let engine = Engine::new().unwrap();
    assert!(engine.description.contains("hardware accelerated"));
    assert!(engine.description.contains("core profile; instanced"));
    assert!((engine.gl.version().major, engine.gl.version().minor) >= (3, 3));
    engine
        .context
        .with_current(|| {
            assert_ne!(
                unsafe { engine.gl.get_parameter_i32(glow::CONTEXT_PROFILE_MASK) }
                    & glow::CONTEXT_CORE_PROFILE_BIT as i32,
                0
            );
            Ok(())
        })
        .unwrap();
}
