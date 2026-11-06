//! GL programs and buffers; all entry points require the owned current CGL context.
use super::{
    atlas::{OpenGlAtlas, PAGE_SIZE},
    context::Context,
    scene::{Frame, Instance},
};
use anyhow::{Result, anyhow, ensure};
use glow::HasContext;
use std::{collections::HashMap, mem, slice};

pub(super) struct Engine {
    pub context: Context,
    pub gl: glow::Context,
    program: glow::NativeProgram,
    buffer: glow::NativeBuffer,
    vertex_array: glow::NativeVertexArray,
    viewport: Option<glow::NativeUniformLocation>,
    textures: HashMap<u32, glow::NativeTexture>,
    pub description: String,
}

impl Engine {
    #[cfg(test)]
    pub(super) fn texture_count(&self) -> usize {
        self.textures.len()
    }
    pub(super) fn new() -> Result<Self> {
        let context = Context::new()?;
        let gl = context.load()?;
        let (program, buffer, vertex_array, viewport, description) =
            context.with_current(|| unsafe {
                let description = super::context::info(&gl)?;
                let program = gl.create_program().map_err(|e| anyhow!(e))?;
                let result = (|| {
                    for (kind, source) in [
                        (glow::VERTEX_SHADER, include_str!("scene.vert")),
                        (glow::FRAGMENT_SHADER, include_str!("scene.frag")),
                    ] {
                        let shader = gl.create_shader(kind).map_err(|e| anyhow!(e))?;
                        gl.shader_source(shader, source);
                        gl.compile_shader(shader);
                        if !gl.get_shader_compile_status(shader) {
                            let log = gl.get_shader_info_log(shader);
                            gl.delete_shader(shader);
                            anyhow::bail!("OpenGL shader {kind} compilation: {log}");
                        }
                        gl.attach_shader(program, shader);
                        gl.delete_shader(shader);
                    }
                    let attributes = [
                        "a_position",
                        "a_bounds",
                        "a_mask",
                        "a_color",
                        "a_border",
                        "a_radii",
                        "a_widths",
                        "a_other",
                        "a_paint",
                        "a_params",
                        "a_element",
                        "a_element_radii",
                        "a_axis_x",
                        "a_axis_y",
                    ];
                    for (ix, name) in attributes.iter().enumerate() {
                        gl.bind_attrib_location(program, ix as u32, name);
                    }
                    gl.link_program(program);
                    ensure!(
                        gl.get_program_link_status(program),
                        "OpenGL shader link: {}",
                        gl.get_program_info_log(program)
                    );
                    let buffer = gl.create_buffer().map_err(|e| anyhow!(e))?;
                    let vertex_array = match gl.create_vertex_array() {
                        Ok(vao) => vao,
                        Err(error) => {
                            gl.delete_buffer(buffer);
                            return Err(anyhow!(error));
                        }
                    };
                    let viewport = gl.get_uniform_location(program, "viewport");
                    gl.use_program(Some(program));
                    gl.uniform_1_i32(gl.get_uniform_location(program, "atlas").as_ref(), 0);
                    gl.use_program(None);
                    Ok((program, buffer, vertex_array, viewport, description))
                })();
                if result.is_err() {
                    gl.delete_program(program);
                }
                result
            })?;
        Ok(Self {
            context,
            gl,
            program,
            buffer,
            vertex_array,
            viewport,
            textures: HashMap::new(),
            description,
        })
    }

    pub(super) unsafe fn draw(
        &mut self,
        frame: &Frame,
        atlas: &OpenGlAtlas,
        width: i32,
        height: i32,
        opaque: bool,
    ) -> Result<()> {
        ensure!(width > 0 && height > 0, "empty OpenGL viewport");
        // SAFETY: caller holds the context current and locked for the whole frame.
        unsafe {
            let gl = &self.gl;
            gl.active_texture(glow::TEXTURE0);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
            gl.pixel_store_i32(glow::UNPACK_SKIP_ROWS, 0);
            gl.pixel_store_i32(glow::UNPACK_SKIP_PIXELS, 0);
            for upload in atlas.take_uploads() {
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.textures.entry(upload.page)
                {
                    let texture = gl.create_texture().map_err(|e| anyhow!(e))?;
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MIN_FILTER,
                        glow::LINEAR as i32,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MAG_FILTER,
                        glow::LINEAR as i32,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_WRAP_S,
                        glow::CLAMP_TO_EDGE as i32,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_WRAP_T,
                        glow::CLAMP_TO_EDGE as i32,
                    );
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA8 as i32,
                        PAGE_SIZE,
                        PAGE_SIZE,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(None),
                    );
                    entry.insert(texture);
                }
                gl.bind_texture(glow::TEXTURE_2D, Some(self.textures[&upload.page]));
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    upload.origin[0],
                    upload.origin[1],
                    upload.size[0],
                    upload.size[1],
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&upload.pixels)),
                );
            }
            gl.viewport(0, 0, width, height);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::SCISSOR_TEST);
            // CAOpenGLLayer does not specify the state on entry.
            gl.disable(glow::RASTERIZER_DISCARD);
            gl.disable(glow::FRAMEBUFFER_SRGB);
            gl.disable(glow::PRIMITIVE_RESTART);
            gl.disable(glow::SAMPLE_MASK);
            gl.bind_sampler(0, None);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::COLOR_LOGIC_OP);
            gl.disable(glow::SAMPLE_ALPHA_TO_COVERAGE);
            gl.disable(glow::SAMPLE_COVERAGE);
            gl.polygon_mode(glow::FRONT_AND_BACK, glow::FILL);
            gl.color_mask(true, true, true, true);
            gl.clear_color(0., 0., 0., if opaque { 1. } else { 0. });
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.use_program(Some(self.program));
            gl.uniform_2_f32(self.viewport.as_ref(), width as f32, height as f32);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.bind_vertex_array(Some(self.vertex_array));
            let stride = mem::size_of::<Instance>();
            let bytes = slice::from_raw_parts(
                frame.instances.as_ptr().cast::<u8>(),
                frame.instances.len() * stride,
            );
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            for ix in 0..14 {
                gl.enable_vertex_attrib_array(ix);
            }
            for batch in &frame.batches {
                let texture = batch
                    .page
                    .and_then(|page| self.textures.get(&page).copied());
                ensure!(
                    batch.page.is_none() || texture.is_some(),
                    "OpenGL atlas texture is missing"
                );
                gl.bind_texture(glow::TEXTURE_2D, texture);
                // Core 3.3 has no base-instance draw. Offset the attributes
                // for each batch, keeping the original scene paint order.
                let offset = batch.first as usize * stride;
                for ix in 0..14 {
                    gl.vertex_attrib_pointer_f32(
                        ix,
                        4,
                        glow::FLOAT,
                        false,
                        stride as i32,
                        (offset + ix as usize * 16)
                            .try_into()
                            .map_err(|_| anyhow!("OpenGL batch offset exceeds i32"))?,
                    );
                    gl.vertex_attrib_divisor(ix, 1);
                }
                gl.draw_arrays_instanced(
                    glow::TRIANGLES,
                    0,
                    batch.vertices_per_instance,
                    batch.count,
                );
            }
            for ix in 0..14 {
                gl.disable_vertex_attrib_array(ix);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            let error = gl.get_error();
            ensure!(error == glow::NO_ERROR, "OpenGL frame error: {error:#x}");
        }
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.context.with_current(|| {
            unsafe {
                for (_, texture) in self.textures.drain() {
                    self.gl.delete_texture(texture);
                }
                self.gl.delete_buffer(self.buffer);
                self.gl.delete_vertex_array(self.vertex_array);
                self.gl.delete_program(self.program);
            }
            Ok(())
        });
    }
}
