//! Encode a borrowed scene as owned vertices or affine instances, in paint order.
use super::atlas::PAGE_SIZE;
use gpui::{
    AtlasTile, Background, BorderStyle, Bounds, Corners, Hsla, PrimitiveBatch, Rgba, ScaledPixels,
    Scene, TransformationMatrix,
};

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(super) struct Vertex {
    pub position: [f32; 4],
    pub bounds: [f32; 4],
    pub mask: [f32; 4],
    pub color: [f32; 4],
    pub border: [f32; 4],
    pub radii: [f32; 4],
    pub widths: [f32; 4],
    pub other: [f32; 4],
    pub paint: [f32; 4],
    pub params: [f32; 4],
    pub element: [f32; 4],
    pub element_radii: [f32; 4],
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub(super) struct Instance {
    pub vertex: Vertex,
    pub axis_x: [f32; 4],
    pub axis_y: [f32; 4],
}
impl Instance {
    fn new(mut vertex: Vertex, positions: [[f32; 4]; 3]) -> Self {
        vertex.position = positions[0];
        Self {
            vertex,
            axis_x: std::array::from_fn(|i| positions[1][i] - positions[0][i]),
            axis_y: std::array::from_fn(|i| positions[2][i] - positions[0][i]),
        }
    }
}

pub(super) struct Batch {
    pub page: Option<u32>,
    pub first: i32,
    pub count: i32,
    pub vertices_per_instance: i32,
}
#[derive(Default)]
pub(super) struct Frame {
    pub instances: Vec<Instance>,
    pub batches: Vec<Batch>,
}

pub(super) fn rgba(c: Hsla) -> [f32; 4] {
    let c = Rgba::from(c);
    [c.r, c.g, c.b, c.a]
}
fn rect(b: Bounds<ScaledPixels>) -> [f32; 4] {
    [b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0]
}
fn radii(c: Corners<ScaledPixels>) -> [f32; 4] {
    [
        c.top_left.0,
        c.top_right.0,
        c.bottom_right.0,
        c.bottom_left.0,
    ]
}

impl Vertex {
    fn fill(&mut self, background: Background) {
        if let Some(c) = background.as_solid() {
            self.color = rgba(c);
            return;
        }
        let v = serde_json::to_value(background).expect("serialize GPUI background");
        let color = |v: &serde_json::Value| {
            rgba(serde_json::from_value(v.clone()).expect("GPUI background color"))
        };
        let dimension = v["gradient_angle_or_pattern_height"].as_f64().unwrap_or(0.) as f32;
        if v["tag"] == "LinearGradient" {
            self.params[1] = 1.;
            self.color = color(&v["colors"][0]["color"]);
            self.other = color(&v["colors"][1]["color"]);
            self.paint = [
                dimension,
                v["colors"][0]["percentage"].as_f64().unwrap_or(0.) as f32,
                v["colors"][1]["percentage"].as_f64().unwrap_or(1.) as f32,
                if v["color_space"] == "Oklab" { 1. } else { 0. },
            ];
        } else {
            self.params[1] = if v["tag"] == "Checkerboard" { 3. } else { 2. };
            self.color = color(&v["solid"]);
            self.paint[0] = dimension;
        }
    }
}

impl Frame {
    fn batch(&mut self, page: Option<u32>, count: i32, vertices_per_instance: i32) {
        let first = self.instances.len() as i32 - count;
        if let Some(last) = self.batches.last_mut() {
            if last.page == page
                && last.vertices_per_instance == vertices_per_instance
                && last.first + last.count == first
            {
                last.count += count;
                return;
            }
        }
        self.batches.push(Batch {
            page,
            first,
            count,
            vertices_per_instance,
        });
    }
    fn rectangle(
        &mut self,
        v: Vertex,
        b: [f32; 4],
        tile: Option<AtlasTile>,
        transform: TransformationMatrix,
    ) {
        let tex = tile.map(|t| {
            [
                t.bounds.origin.x.0 as f32,
                t.bounds.origin.y.0 as f32,
                t.bounds.size.width.0 as f32,
                t.bounds.size.height.0 as f32,
            ]
        });
        let at = |x, y| {
            let p = [b[0] + b[2] * x, b[1] + b[3] * y];
            let mut position = [
                transform.translation[0]
                    + transform.rotation_scale[0][0] * p[0]
                    + transform.rotation_scale[0][1] * p[1],
                transform.translation[1]
                    + transform.rotation_scale[1][0] * p[0]
                    + transform.rotation_scale[1][1] * p[1],
                0.,
                0.,
            ];
            if let Some(t) = tex {
                position[2] = (t[0] + t[2] * x) / PAGE_SIZE as f32;
                position[3] = (t[1] + t[3] * y) / PAGE_SIZE as f32;
            }
            position
        };
        let page = tile.map(|t| t.texture_id.index);
        self.instances
            .push(Instance::new(v, [at(0., 0.), at(1., 0.), at(0., 1.)]));
        self.batch(page, 1, 6);
    }
    pub(super) fn triangle(&mut self, vertex: Vertex, positions: [[f32; 4]; 3]) {
        self.instances.push(Instance::new(vertex, positions));
        self.batch(None, 1, 3);
    }
    pub(super) fn encode(scene: &Scene) -> Self {
        let mut frame = Self::default();
        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Quads(range) => {
                    for q in &scene.quads[range] {
                        let mut v = Vertex {
                            bounds: rect(q.bounds),
                            mask: rect(q.content_mask.bounds),
                            border: rgba(q.border_color),
                            radii: radii(q.corner_radii),
                            widths: [
                                q.border_widths.top.0,
                                q.border_widths.right.0,
                                q.border_widths.bottom.0,
                                q.border_widths.left.0,
                            ],
                            ..Default::default()
                        };
                        v.params[2] = if q.border_style == BorderStyle::Dashed {
                            1.
                        } else {
                            0.
                        };
                        v.fill(q.background);
                        frame.rectangle(v, v.bounds, None, TransformationMatrix::unit());
                    }
                }
                PrimitiveBatch::MonochromeSprites { range, .. } => {
                    for s in &scene.monochrome_sprites[range] {
                        let v = Vertex {
                            bounds: rect(s.bounds),
                            mask: rect(s.content_mask.bounds),
                            color: rgba(s.color),
                            params: [1., 0., 0., 1.],
                            ..Default::default()
                        };
                        frame.rectangle(v, v.bounds, Some(s.tile), s.transformation);
                    }
                }
                PrimitiveBatch::SubpixelSprites { range, .. } => {
                    for s in &scene.subpixel_sprites[range] {
                        let v = Vertex {
                            bounds: rect(s.bounds),
                            mask: rect(s.content_mask.bounds),
                            color: rgba(s.color),
                            params: [1., 0., 0., 1.],
                            ..Default::default()
                        };
                        frame.rectangle(v, v.bounds, Some(s.tile), s.transformation);
                    }
                }
                PrimitiveBatch::PolychromeSprites { range, .. } => {
                    for s in &scene.polychrome_sprites[range] {
                        let v = Vertex {
                            bounds: rect(s.bounds),
                            mask: rect(s.content_mask.bounds),
                            radii: radii(s.corner_radii),
                            params: [
                                2.,
                                0.,
                                if s.grayscale != gpui::PaddedBool32::default() {
                                    1.
                                } else {
                                    0.
                                },
                                s.opacity,
                            ],
                            ..Default::default()
                        };
                        frame.rectangle(v, v.bounds, Some(s.tile), TransformationMatrix::unit());
                    }
                }
                PrimitiveBatch::Paths(range) => {
                    for p in &scene.paths[range] {
                        let mut v = Vertex {
                            bounds: rect(p.bounds),
                            mask: rect(p.content_mask.bounds),
                            params: [3., 0., 0., 0.],
                            ..Default::default()
                        };
                        v.fill(p.color);
                        for triangle in p.vertices.chunks_exact(3) {
                            frame.triangle(
                                v,
                                std::array::from_fn(|i| {
                                    let vertex = &triangle[i];
                                    [
                                        vertex.xy_position.x.0,
                                        vertex.xy_position.y.0,
                                        vertex.st_position.x,
                                        vertex.st_position.y,
                                    ]
                                }),
                            );
                        }
                    }
                }
                PrimitiveBatch::Shadows(range) => {
                    for s in &scene.shadows[range] {
                        let v = Vertex {
                            bounds: rect(s.bounds),
                            mask: rect(s.content_mask.bounds),
                            color: rgba(s.color),
                            radii: radii(s.corner_radii),
                            element: rect(s.element_bounds),
                            element_radii: radii(s.element_corner_radii),
                            params: [4., 0., s.blur_radius.0, s.inset as f32],
                            ..Default::default()
                        };
                        let mut b = v.bounds;
                        let grow = s.blur_radius.0 * 3.;
                        b[0] -= grow;
                        b[1] -= grow;
                        b[2] += grow * 2.;
                        b[3] += grow * 2.;
                        frame.rectangle(v, b, None, TransformationMatrix::unit());
                    }
                }
                PrimitiveBatch::Underlines(range) => {
                    for u in &scene.underlines[range] {
                        let v = Vertex {
                            bounds: rect(u.bounds),
                            mask: rect(u.content_mask.bounds),
                            color: rgba(u.color),
                            params: [
                                5.,
                                0.,
                                u.thickness.0,
                                if u.wavy != gpui::PaddedBool32::default() {
                                    1.
                                } else {
                                    0.
                                },
                            ],
                            ..Default::default()
                        };
                        frame.rectangle(v, v.bounds, None, TransformationMatrix::unit());
                    }
                }
                // Qrow has no video/camera surfaces. Pixel buffers need a separate adapter.
                PrimitiveBatch::Surfaces(_) => {}
            }
        }
        frame
    }
}
