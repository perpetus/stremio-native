use std::{
    ffi::{CStr, c_char, c_int, c_void},
    marker::PhantomData,
    num::NonZeroU32,
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use glow::HasContext;

use crate::ffi::{
    MpvClient, MpvError, MpvOpenGlFbo, MpvOpenGlInitParams, MpvRenderContext, MpvRenderParam,
    RENDER_PARAM_ADVANCED_CONTROL, RENDER_PARAM_API_TYPE, RENDER_PARAM_FLIP_Y,
    RENDER_PARAM_INVALID, RENDER_PARAM_OPENGL_FBO, RENDER_PARAM_OPENGL_INIT_PARAMS,
    RENDER_PARAM_SKIP_RENDERING, RENDER_UPDATE_FRAME,
};

/// OpenGL function resolver supplied by Slint while its context is current.
pub type OpenGlProcAddress<'a> = &'a dyn Fn(&CStr) -> *const c_void;

#[derive(Clone, Copy)]
struct TextureUnitState {
    unit: u32,
    texture_2d: Option<glow::NativeTexture>,
    sampler: Option<glow::NativeSampler>,
}

#[derive(Clone, Copy)]
struct StencilFaceState {
    function: u32,
    reference: i32,
    value_mask: u32,
    write_mask: u32,
    stencil_fail: u32,
    depth_fail: u32,
    depth_pass: u32,
}

struct OpenGlState {
    draw_framebuffer: Option<glow::NativeFramebuffer>,
    read_framebuffer: Option<glow::NativeFramebuffer>,
    renderbuffer: Option<glow::NativeRenderbuffer>,
    program: Option<glow::NativeProgram>,
    vertex_array: Option<glow::NativeVertexArray>,
    array_buffer: Option<glow::NativeBuffer>,
    element_array_buffer: Option<glow::NativeBuffer>,
    pixel_pack_buffer: Option<glow::NativeBuffer>,
    pixel_unpack_buffer: Option<glow::NativeBuffer>,
    active_texture: u32,
    texture_units: Vec<TextureUnitState>,
    viewport: [c_int; 4],
    scissor_box: [c_int; 4],
    clear_color: [f32; 4],
    blend_color: [f32; 4],
    color_mask: [bool; 4],
    blend_equation_rgb: u32,
    blend_equation_alpha: u32,
    blend_source_rgb: u32,
    blend_destination_rgb: u32,
    blend_source_alpha: u32,
    blend_destination_alpha: u32,
    depth_function: u32,
    depth_write_mask: bool,
    cull_face_mode: u32,
    front_face: u32,
    stencil_front: StencilFaceState,
    stencil_back: StencilFaceState,
    pack_alignment: i32,
    unpack_alignment: i32,
    blend: bool,
    cull_face: bool,
    depth_test: bool,
    scissor_test: bool,
    stencil_test: bool,
    dither: bool,
    multisample: bool,
    framebuffer_srgb: bool,
    rasterizer_discard: bool,
    sample_alpha_to_coverage: bool,
    sample_coverage: bool,
    sampler_objects: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OpenGlCapabilities {
    sampler_objects: bool,
    desktop_multisample: bool,
    framebuffer_srgb: bool,
}

impl OpenGlCapabilities {
    fn from_version(major: u32, minor: u32, is_embedded: bool) -> Self {
        Self {
            // Sampler objects are core in OpenGL ES 3.0 and desktop OpenGL 3.3.
            // Slint uses GLES on Windows even for GraphicsAPI::NativeOpenGL.
            sampler_objects: if is_embedded {
                major >= 3
            } else {
                major > 3 || (major == 3 && minor >= 3)
            },
            // These enable flags are desktop GL state. Querying them on GLES can
            // produce GL_INVALID_ENUM and contaminate the shared context.
            desktop_multisample: !is_embedded,
            framebuffer_srgb: !is_embedded && major >= 3,
        }
    }

    fn for_context(gl: &glow::Context) -> Self {
        let version = gl.version();
        Self::from_version(version.major, version.minor, version.is_embedded)
    }
}

fn prepare_for_mpv(gl: &glow::Context) {
    let capabilities = OpenGlCapabilities::for_context(gl);
    // SAFETY: Slint's rendering notifier guarantees that this context is
    // current. The surrounding OpenGlStateGuard restores every changed value.
    unsafe {
        for unit in [glow::TEXTURE0, glow::TEXTURE1] {
            gl.active_texture(unit);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if capabilities.sampler_objects {
                gl.bind_sampler(unit - glow::TEXTURE0, None);
            }
        }
        gl.active_texture(glow::TEXTURE0);
        gl.use_program(None);
        gl.bind_vertex_array(None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::STENCIL_TEST);
        gl.color_mask(true, true, true, true);
        gl.depth_mask(true);
    }
}

impl OpenGlState {
    fn capture(gl: &glow::Context) -> Self {
        let mut viewport = [0; 4];
        let mut scissor_box = [0; 4];
        let mut clear_color = [0.0; 4];
        let mut blend_color = [0.0; 4];
        // SAFETY: Slint guarantees that its OpenGL context is current for the
        // rendering notifier callback.
        unsafe {
            let capabilities = OpenGlCapabilities::for_context(gl);
            gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
            gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor_box);
            gl.get_parameter_f32_slice(glow::COLOR_CLEAR_VALUE, &mut clear_color);
            gl.get_parameter_f32_slice(glow::BLEND_COLOR, &mut blend_color);
            let active_texture = gl.get_parameter_i32(glow::ACTIVE_TEXTURE) as u32;
            let mut texture_units = Vec::with_capacity(3);
            for unit in [glow::TEXTURE0, glow::TEXTURE1, active_texture] {
                if texture_units
                    .iter()
                    .any(|state: &TextureUnitState| state.unit == unit)
                {
                    continue;
                }
                gl.active_texture(unit);
                texture_units.push(TextureUnitState {
                    unit,
                    texture_2d: native_texture(gl.get_parameter_i32(glow::TEXTURE_BINDING_2D)),
                    sampler: capabilities
                        .sampler_objects
                        .then(|| native_sampler(gl.get_parameter_i32(glow::SAMPLER_BINDING)))
                        .flatten(),
                });
            }
            gl.active_texture(active_texture);
            Self {
                draw_framebuffer: native_framebuffer(
                    gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING),
                ),
                read_framebuffer: native_framebuffer(
                    gl.get_parameter_i32(glow::READ_FRAMEBUFFER_BINDING),
                ),
                renderbuffer: native_renderbuffer(gl.get_parameter_i32(glow::RENDERBUFFER_BINDING)),
                program: native_program(gl.get_parameter_i32(glow::CURRENT_PROGRAM)),
                vertex_array: native_vertex_array(gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING)),
                array_buffer: native_buffer(gl.get_parameter_i32(glow::ARRAY_BUFFER_BINDING)),
                element_array_buffer: native_buffer(
                    gl.get_parameter_i32(glow::ELEMENT_ARRAY_BUFFER_BINDING),
                ),
                pixel_pack_buffer: native_buffer(
                    gl.get_parameter_i32(glow::PIXEL_PACK_BUFFER_BINDING),
                ),
                pixel_unpack_buffer: native_buffer(
                    gl.get_parameter_i32(glow::PIXEL_UNPACK_BUFFER_BINDING),
                ),
                active_texture,
                texture_units,
                viewport,
                scissor_box,
                clear_color,
                blend_color,
                color_mask: gl.get_parameter_bool_array(glow::COLOR_WRITEMASK),
                blend_equation_rgb: gl.get_parameter_i32(glow::BLEND_EQUATION_RGB) as u32,
                blend_equation_alpha: gl.get_parameter_i32(glow::BLEND_EQUATION_ALPHA) as u32,
                blend_source_rgb: gl.get_parameter_i32(glow::BLEND_SRC_RGB) as u32,
                blend_destination_rgb: gl.get_parameter_i32(glow::BLEND_DST_RGB) as u32,
                blend_source_alpha: gl.get_parameter_i32(glow::BLEND_SRC_ALPHA) as u32,
                blend_destination_alpha: gl.get_parameter_i32(glow::BLEND_DST_ALPHA) as u32,
                depth_function: gl.get_parameter_i32(glow::DEPTH_FUNC) as u32,
                depth_write_mask: gl.get_parameter_bool(glow::DEPTH_WRITEMASK),
                cull_face_mode: gl.get_parameter_i32(glow::CULL_FACE_MODE) as u32,
                front_face: gl.get_parameter_i32(glow::FRONT_FACE) as u32,
                stencil_front: capture_stencil_face(gl, false),
                stencil_back: capture_stencil_face(gl, true),
                pack_alignment: gl.get_parameter_i32(glow::PACK_ALIGNMENT),
                unpack_alignment: gl.get_parameter_i32(glow::UNPACK_ALIGNMENT),
                blend: gl.is_enabled(glow::BLEND),
                cull_face: gl.is_enabled(glow::CULL_FACE),
                depth_test: gl.is_enabled(glow::DEPTH_TEST),
                scissor_test: gl.is_enabled(glow::SCISSOR_TEST),
                stencil_test: gl.is_enabled(glow::STENCIL_TEST),
                dither: gl.is_enabled(glow::DITHER),
                multisample: capabilities.desktop_multisample && gl.is_enabled(glow::MULTISAMPLE),
                framebuffer_srgb: capabilities.framebuffer_srgb
                    && gl.is_enabled(glow::FRAMEBUFFER_SRGB),
                rasterizer_discard: gl.is_enabled(glow::RASTERIZER_DISCARD),
                sample_alpha_to_coverage: gl.is_enabled(glow::SAMPLE_ALPHA_TO_COVERAGE),
                sample_coverage: gl.is_enabled(glow::SAMPLE_COVERAGE),
                sampler_objects: capabilities.sampler_objects,
            }
        }
    }

    fn restore(self, gl: &glow::Context) {
        // SAFETY: These values were captured from the same current context
        // immediately before MPV rendering.
        unsafe {
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, self.draw_framebuffer);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, self.read_framebuffer);
            gl.bind_renderbuffer(glow::RENDERBUFFER, self.renderbuffer);
            gl.viewport(
                self.viewport[0],
                self.viewport[1],
                self.viewport[2],
                self.viewport[3],
            );
            gl.scissor(
                self.scissor_box[0],
                self.scissor_box[1],
                self.scissor_box[2],
                self.scissor_box[3],
            );
            gl.use_program(self.program);
            gl.bind_vertex_array(self.vertex_array);
            gl.bind_buffer(glow::ARRAY_BUFFER, self.array_buffer);
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, self.element_array_buffer);
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, self.pixel_pack_buffer);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, self.pixel_unpack_buffer);
            for texture_unit in self.texture_units {
                gl.active_texture(texture_unit.unit);
                gl.bind_texture(glow::TEXTURE_2D, texture_unit.texture_2d);
                if self.sampler_objects {
                    gl.bind_sampler(texture_unit.unit - glow::TEXTURE0, texture_unit.sampler);
                }
            }
            gl.active_texture(self.active_texture);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, self.pack_alignment);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, self.unpack_alignment);
            gl.blend_equation_separate(self.blend_equation_rgb, self.blend_equation_alpha);
            gl.blend_func_separate(
                self.blend_source_rgb,
                self.blend_destination_rgb,
                self.blend_source_alpha,
                self.blend_destination_alpha,
            );
            gl.blend_color(
                self.blend_color[0],
                self.blend_color[1],
                self.blend_color[2],
                self.blend_color[3],
            );
            gl.clear_color(
                self.clear_color[0],
                self.clear_color[1],
                self.clear_color[2],
                self.clear_color[3],
            );
            gl.color_mask(
                self.color_mask[0],
                self.color_mask[1],
                self.color_mask[2],
                self.color_mask[3],
            );
            gl.depth_func(self.depth_function);
            gl.depth_mask(self.depth_write_mask);
            gl.cull_face(self.cull_face_mode);
            gl.front_face(self.front_face);
            restore_stencil_face(gl, glow::FRONT, self.stencil_front);
            restore_stencil_face(gl, glow::BACK, self.stencil_back);
            restore_capability(gl, glow::BLEND, self.blend);
            restore_capability(gl, glow::CULL_FACE, self.cull_face);
            restore_capability(gl, glow::DEPTH_TEST, self.depth_test);
            restore_capability(gl, glow::SCISSOR_TEST, self.scissor_test);
            restore_capability(gl, glow::STENCIL_TEST, self.stencil_test);
            restore_capability(gl, glow::DITHER, self.dither);
            let capabilities = OpenGlCapabilities::for_context(gl);
            if capabilities.desktop_multisample {
                restore_capability(gl, glow::MULTISAMPLE, self.multisample);
            }
            if capabilities.framebuffer_srgb {
                restore_capability(gl, glow::FRAMEBUFFER_SRGB, self.framebuffer_srgb);
            }
            restore_capability(gl, glow::RASTERIZER_DISCARD, self.rasterizer_discard);
            restore_capability(
                gl,
                glow::SAMPLE_ALPHA_TO_COVERAGE,
                self.sample_alpha_to_coverage,
            );
            restore_capability(gl, glow::SAMPLE_COVERAGE, self.sample_coverage);
        }
    }
}

struct OpenGlStateGuard<'a> {
    gl: &'a glow::Context,
    state: Option<OpenGlState>,
}

impl<'a> OpenGlStateGuard<'a> {
    fn new(gl: &'a glow::Context) -> Self {
        Self {
            gl,
            state: Some(OpenGlState::capture(gl)),
        }
    }
}

impl Drop for OpenGlStateGuard<'_> {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            state.restore(self.gl);
        }
    }
}

unsafe fn capture_stencil_face(gl: &glow::Context, back: bool) -> StencilFaceState {
    let parameters = if back {
        [
            glow::STENCIL_BACK_FUNC,
            glow::STENCIL_BACK_REF,
            glow::STENCIL_BACK_VALUE_MASK,
            glow::STENCIL_BACK_WRITEMASK,
            glow::STENCIL_BACK_FAIL,
            glow::STENCIL_BACK_PASS_DEPTH_FAIL,
            glow::STENCIL_BACK_PASS_DEPTH_PASS,
        ]
    } else {
        [
            glow::STENCIL_FUNC,
            glow::STENCIL_REF,
            glow::STENCIL_VALUE_MASK,
            glow::STENCIL_WRITEMASK,
            glow::STENCIL_FAIL,
            glow::STENCIL_PASS_DEPTH_FAIL,
            glow::STENCIL_PASS_DEPTH_PASS,
        ]
    };
    // SAFETY: All queried values are scalar OpenGL state from the current
    // context guaranteed by Slint's rendering notifier.
    unsafe {
        StencilFaceState {
            function: gl.get_parameter_i32(parameters[0]) as u32,
            reference: gl.get_parameter_i32(parameters[1]),
            value_mask: gl.get_parameter_i32(parameters[2]) as u32,
            write_mask: gl.get_parameter_i32(parameters[3]) as u32,
            stencil_fail: gl.get_parameter_i32(parameters[4]) as u32,
            depth_fail: gl.get_parameter_i32(parameters[5]) as u32,
            depth_pass: gl.get_parameter_i32(parameters[6]) as u32,
        }
    }
}

unsafe fn restore_stencil_face(gl: &glow::Context, face: u32, state: StencilFaceState) {
    // SAFETY: The values were captured from this same current OpenGL context.
    unsafe {
        gl.stencil_func_separate(face, state.function, state.reference, state.value_mask);
        gl.stencil_mask_separate(face, state.write_mask);
        gl.stencil_op_separate(face, state.stencil_fail, state.depth_fail, state.depth_pass);
    }
}

unsafe fn restore_capability(gl: &glow::Context, capability: u32, enabled: bool) {
    if enabled {
        // SAFETY: `capability` is a valid OpenGL capability captured from this
        // same current context immediately before MPV rendering.
        unsafe { gl.enable(capability) };
    } else {
        // SAFETY: See the enabled branch above.
        unsafe { gl.disable(capability) };
    }
}

fn native_framebuffer(value: c_int) -> Option<glow::NativeFramebuffer> {
    NonZeroU32::new(value as u32).map(glow::NativeFramebuffer)
}

fn native_texture(value: c_int) -> Option<glow::NativeTexture> {
    NonZeroU32::new(value as u32).map(glow::NativeTexture)
}

fn native_buffer(value: c_int) -> Option<glow::NativeBuffer> {
    NonZeroU32::new(value as u32).map(glow::NativeBuffer)
}

fn native_renderbuffer(value: c_int) -> Option<glow::NativeRenderbuffer> {
    NonZeroU32::new(value as u32).map(glow::NativeRenderbuffer)
}

fn native_program(value: c_int) -> Option<glow::NativeProgram> {
    NonZeroU32::new(value as u32).map(glow::NativeProgram)
}

fn native_vertex_array(value: c_int) -> Option<glow::NativeVertexArray> {
    NonZeroU32::new(value as u32).map(glow::NativeVertexArray)
}

fn native_sampler(value: c_int) -> Option<glow::NativeSampler> {
    NonZeroU32::new(value as u32).map(glow::NativeSampler)
}

struct OpenGlRenderTarget {
    framebuffer: glow::NativeFramebuffer,
    texture: glow::NativeTexture,
    width: i32,
    height: i32,
}

struct OpenGlRenderTargets {
    slots: [OpenGlRenderTarget; 2],
    next_render_index: usize,
}

impl OpenGlRenderTargets {
    fn has_size(&self, width: i32, height: i32) -> bool {
        self.slots[0].width == width && self.slots[0].height == height
    }
}

/// Borrowed OpenGL texture information for compositing the MPV frame in Slint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VideoTexture {
    texture_id: NonZeroU32,
    width: u32,
    height: u32,
}

impl VideoTexture {
    pub fn texture_id(self) -> NonZeroU32 {
        self.texture_id
    }

    pub fn width(self) -> u32 {
        self.width
    }

    pub fn height(self) -> u32 {
        self.height
    }
}

fn create_render_target(
    gl: &glow::Context,
    width: i32,
    height: i32,
) -> Result<OpenGlRenderTarget, MpvError> {
    let _state_guard = OpenGlStateGuard::new(gl);
    (|| {
        // SAFETY: All resources are created in Slint's current OpenGL context.
        let texture = unsafe { gl.create_texture() }.map_err(MpvError::OpenGl)?;
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
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
            // libmpv's render API does not promise a meaningful alpha channel.
            // Slint composites borrowed textures using alpha, so force the
            // sampled video surface to be opaque regardless of the decoder's
            // output. This does not alter the RGB video data in the FBO.
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_SWIZZLE_A, glow::ONE as i32);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
        }

        let framebuffer = match unsafe { gl.create_framebuffer() } {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                // SAFETY: `texture` was created above in the current context.
                unsafe { gl.delete_texture(texture) };
                return Err(MpvError::OpenGl(error));
            }
        };
        // SAFETY: The framebuffer and texture belong to the current context.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
        }
        let status = unsafe { gl.check_framebuffer_status(glow::FRAMEBUFFER) };
        if status != glow::FRAMEBUFFER_COMPLETE {
            // SAFETY: Both resources were created in this current context.
            unsafe {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
            }
            return Err(MpvError::IncompleteOpenGlFramebuffer { status });
        }

        Ok(OpenGlRenderTarget {
            framebuffer,
            texture,
            width,
            height,
        })
    })()
}

fn delete_render_target(gl: &glow::Context, target: OpenGlRenderTarget) {
    // SAFETY: The target was created in this current OpenGL context and is no
    // longer borrowed by the caller when this function takes ownership.
    unsafe {
        gl.delete_framebuffer(target.framebuffer);
        gl.delete_texture(target.texture);
    }
}

fn create_render_targets(
    gl: &glow::Context,
    width: i32,
    height: i32,
) -> Result<OpenGlRenderTargets, MpvError> {
    let first = create_render_target(gl, width, height)?;
    let second = match create_render_target(gl, width, height) {
        Ok(target) => target,
        Err(error) => {
            delete_render_target(gl, first);
            return Err(error);
        }
    };
    Ok(OpenGlRenderTargets {
        slots: [first, second],
        next_render_index: 0,
    })
}

#[derive(Clone)]
pub struct RenderSource {
    client: Arc<MpvClient>,
}

impl RenderSource {
    pub(crate) fn new(client: Arc<MpvClient>) -> Self {
        Self { client }
    }

    /// Creates the MPV OpenGL render context on the GUI thread.
    ///
    /// The caller must invoke this while Slint's OpenGL context is current and
    /// retain the returned context on that same thread.
    pub fn create_context(
        &self,
        resolver: OpenGlProcAddress<'_>,
        request_redraw: impl FnMut() + Send + 'static,
    ) -> Result<RenderContext, MpvError> {
        ResolverBridge::create(&self.client, resolver, request_redraw)
    }
}

struct RedrawCallback {
    pending: AtomicBool,
    callback: Mutex<Box<dyn FnMut() + Send>>,
}

struct ResolverBridge<'a> {
    resolver: OpenGlProcAddress<'a>,
}

impl<'a> ResolverBridge<'a> {
    fn create(
        client: &Arc<MpvClient>,
        resolver: OpenGlProcAddress<'a>,
        request_redraw: impl FnMut() + Send + 'static,
    ) -> Result<RenderContext, MpvError> {
        // SAFETY: Slint invokes this function with its OpenGL context current,
        // and its resolver remains valid for synchronous symbol loading.
        let gl =
            Rc::new(unsafe { glow::Context::from_loader_function_cstr(|name| resolver(name)) });
        let gl_state = OpenGlStateGuard::new(&gl);
        prepare_for_mpv(&gl);
        let mut bridge = Self { resolver };
        let mut init_params = MpvOpenGlInitParams {
            get_proc_address: Some(resolve_open_gl),
            get_proc_address_ctx: (&mut bridge as *mut Self).cast(),
        };
        let mut advanced_control: c_int = 1;
        let api_type = c"opengl";
        let mut params = [
            MpvRenderParam {
                param_type: RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr().cast_mut().cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: (&mut init_params as *mut MpvOpenGlInitParams).cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_ADVANCED_CONTROL,
                data: (&mut advanced_control as *mut c_int).cast(),
            },
            MpvRenderParam {
                param_type: RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let mut raw_context = std::ptr::null_mut();
        // SAFETY: Slint's context is current. Parameter storage and the resolver
        // bridge remain valid for the synchronous context creation call.
        client.api.result(unsafe {
            (client.api.render_create)(&mut raw_context, client.handle(), params.as_mut_ptr())
        })?;
        let context = NonNull::new(raw_context).ok_or(MpvError::NullRenderContext)?;
        let redraw = Box::new(RedrawCallback {
            pending: AtomicBool::new(false),
            callback: Mutex::new(Box::new(request_redraw)),
        });
        // SAFETY: `redraw` remains boxed at a stable address until Drop removes
        // the callback before freeing it.
        unsafe {
            (client.api.render_set_update_callback)(
                context.as_ptr(),
                Some(render_update),
                (&*redraw as *const RedrawCallback).cast_mut().cast(),
            )
        };
        // Context creation is permitted to alter OpenGL state, including
        // disabling dithering. Restore Slint's exact state before returning.
        drop(gl_state);

        Ok(RenderContext {
            context,
            client: client.clone(),
            gl,
            render_targets: None,
            force_render: false,
            frame_pending: false,
            last_gl_error: None,
            _redraw: redraw,
            _not_send: PhantomData,
        })
    }
}

unsafe extern "C" fn resolve_open_gl(context: *mut c_void, name: *const c_char) -> *mut c_void {
    if context.is_null() || name.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: Context points to ResolverBridge during the synchronous create
    // call, and MPV supplies a null-terminated GL function name.
    let bridge = unsafe { &*(context as *const ResolverBridge<'_>) };
    // SAFETY: Name validity is guaranteed by MPV's render API.
    let name = unsafe { CStr::from_ptr(name) };
    (bridge.resolver)(name).cast_mut()
}

unsafe extern "C" fn render_update(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    // SAFETY: Context points to RedrawCallback until the callback is removed.
    let callback = unsafe { &*(context as *const RedrawCallback) };
    if callback.pending.swap(true, Ordering::AcqRel) {
        return;
    }
    let redraw_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut redraw = callback
            .callback
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        redraw();
    }));
    if redraw_result.is_err() {
        callback.pending.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenGlDiagnostics {
    pub vendor: String,
    pub renderer: String,
    pub version: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderOutcome {
    NoFrame,
    Rendered {
        texture: VideoTexture,
        frame_ready: bool,
    },
}

/// MPV render context owned and used exclusively by Slint's GUI thread.
pub struct RenderContext {
    context: NonNull<MpvRenderContext>,
    client: Arc<MpvClient>,
    gl: Rc<glow::Context>,
    render_targets: Option<OpenGlRenderTargets>,
    force_render: bool,
    frame_pending: bool,
    last_gl_error: Option<u32>,
    _redraw: Box<RedrawCallback>,
    _not_send: PhantomData<Rc<()>>,
}

impl RenderContext {
    pub fn open_gl_diagnostics(&self) -> OpenGlDiagnostics {
        // SAFETY: This method is called from Slint's rendering notifier while
        // the same OpenGL context used to create this renderer is current.
        unsafe {
            OpenGlDiagnostics {
                vendor: self.gl.get_parameter_string(glow::VENDOR),
                renderer: self.gl.get_parameter_string(glow::RENDERER),
                version: self.gl.get_parameter_string(glow::VERSION),
            }
        }
    }

    /// Allocates the double-buffered private video textures on first use.
    ///
    /// Returns `true` when the targets were created or recreated. Both texture
    /// IDs remain owned by this render context until resize or teardown.
    pub fn ensure_video_textures(&mut self, width: i32, height: i32) -> Result<bool, MpvError> {
        if width <= 0 || height <= 0 {
            return Ok(false);
        }
        if self
            .render_targets
            .as_ref()
            .is_some_and(|targets| targets.has_size(width, height))
        {
            return Ok(false);
        }
        let targets = create_render_targets(&self.gl, width, height)?;
        if let Some(previous) = self.render_targets.replace(targets) {
            for target in previous.slots {
                delete_render_target(&self.gl, target);
            }
        }
        self.force_render = true;
        self.frame_pending = false;
        Ok(true)
    }

    pub fn has_video_textures(&self) -> bool {
        self.render_targets.is_some()
    }

    /// Drains libmpv's advanced-control update callback.
    ///
    /// This must be called for every requested redraw even while the player is
    /// hidden. Hidden frames are explicitly skipped so libmpv can continue its
    /// render lifecycle without presenting stale work later.
    pub fn process_updates(&mut self, render_visible: bool) -> Result<bool, MpvError> {
        if !self._redraw.pending.swap(false, Ordering::AcqRel) {
            return Ok(false);
        }
        let update_flags = {
            let _state_guard = OpenGlStateGuard::new(&self.gl);
            prepare_for_mpv(&self.gl);
            // SAFETY: Called on the render-context owner thread while Slint's
            // OpenGL context is current.
            unsafe { (self.client.api.render_update)(self.context.as_ptr()) }
        };
        let has_frame = update_flags & RENDER_UPDATE_FRAME != 0;
        if !has_frame {
            return Ok(false);
        }
        if render_visible {
            self.frame_pending = true;
        } else {
            self.skip_pending_frame()?;
        }
        Ok(true)
    }

    fn skip_pending_frame(&mut self) -> Result<(), MpvError> {
        let result = {
            let _state_guard = OpenGlStateGuard::new(&self.gl);
            prepare_for_mpv(&self.gl);
            let mut skip: c_int = 1;
            let mut params = [
                MpvRenderParam {
                    param_type: RENDER_PARAM_SKIP_RENDERING,
                    data: (&mut skip as *mut c_int).cast(),
                },
                MpvRenderParam {
                    param_type: RENDER_PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            // SAFETY: The same OpenGL context used to create the render context
            // is current. MPV explicitly permits skip rendering without an FBO.
            self.client.api.result(unsafe {
                (self.client.api.render)(self.context.as_ptr(), params.as_mut_ptr())
            })
        };
        self.frame_pending = false;
        result
    }

    /// Renders the next MPV frame into the back RGBA texture.
    ///
    /// The caller should publish the returned texture to Slint and request one
    /// more redraw. Alternating targets prevents Slint from sampling the same
    /// texture that libmpv is updating.
    pub fn render(&mut self) -> Result<RenderOutcome, MpvError> {
        self.process_updates(true)?;
        let Some(targets) = self.render_targets.as_ref() else {
            return Ok(RenderOutcome::NoFrame);
        };
        if !self.frame_pending && !self.force_render {
            return Ok(RenderOutcome::NoFrame);
        }
        let frame_ready = self.frame_pending;
        let render_index = targets.next_render_index;
        let target = &targets.slots[render_index];
        let framebuffer_id = target.framebuffer.0.get() as c_int;
        let texture = VideoTexture {
            texture_id: target.texture.0,
            width: target.width as u32,
            height: target.height as u32,
        };
        let width = target.width;
        let height = target.height;

        let result = {
            let _state_guard = OpenGlStateGuard::new(&self.gl);
            prepare_for_mpv(&self.gl);
            // SAFETY: The target belongs to this current context. Clearing
            // first guarantees deterministic black letterboxing.
            unsafe {
                self.gl
                    .bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
                self.gl.viewport(0, 0, width, height);
                self.gl.disable(glow::SCISSOR_TEST);
                self.gl.color_mask(true, true, true, true);
                self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
                self.gl.clear(glow::COLOR_BUFFER_BIT);
            }
            let mut framebuffer = MpvOpenGlFbo {
                fbo: framebuffer_id,
                width,
                height,
                internal_format: glow::RGBA8 as c_int,
            };
            // Slint's borrowed OpenGL texture path samples this FBO in the same
            // orientation produced by libmpv. Requesting MPV's optional flip
            // here applies a second coordinate conversion and turns the frame
            // upside down (confirmed on the Windows FemtoVG/Skia render path).
            let mut flip_y: c_int = 0;
            let mut params = [
                MpvRenderParam {
                    param_type: RENDER_PARAM_OPENGL_FBO,
                    data: (&mut framebuffer as *mut MpvOpenGlFbo).cast(),
                },
                MpvRenderParam {
                    param_type: RENDER_PARAM_FLIP_Y,
                    data: (&mut flip_y as *mut c_int).cast(),
                },
                MpvRenderParam {
                    param_type: RENDER_PARAM_INVALID,
                    data: std::ptr::null_mut(),
                },
            ];
            // SAFETY: Called on the render-context owner thread while Slint
            // keeps the framebuffer and GL context current.
            self.client.api.result(unsafe {
                (self.client.api.render)(self.context.as_ptr(), params.as_mut_ptr())
            })
        };
        // SAFETY: The current context remains owned by Slint's rendering
        // callback. Reading the error flag does not mutate render resources.
        let gl_error = unsafe { self.gl.get_error() };
        self.last_gl_error = (gl_error != glow::NO_ERROR).then_some(gl_error);
        result.map(|()| {
            if let Some(targets) = self.render_targets.as_mut() {
                targets.next_render_index = (render_index + 1) % targets.slots.len();
            }
            self.force_render = false;
            self.frame_pending = false;
            RenderOutcome::Rendered {
                texture,
                frame_ready,
            }
        })
    }

    /// Returns and clears the most recent OpenGL error observed after rendering.
    pub fn take_gl_error(&mut self) -> Option<u32> {
        self.last_gl_error.take()
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        // SAFETY: Drop runs on the GUI thread. Removing the update callback
        // prevents MPV from touching `_redraw` before the context is freed.
        unsafe {
            (self.client.api.render_set_update_callback)(
                self.context.as_ptr(),
                None,
                std::ptr::null_mut(),
            );
            (self.client.api.render_free)(self.context.as_ptr());
        }
        if let Some(targets) = self.render_targets.take() {
            for target in targets.slots {
                delete_render_target(&self.gl, target);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OpenGlCapabilities;

    #[test]
    fn sampler_objects_are_supported_by_opengl_es_3() {
        let capabilities = OpenGlCapabilities::from_version(3, 0, true);

        assert!(capabilities.sampler_objects);
    }

    #[test]
    fn sampler_objects_require_desktop_opengl_3_3() {
        let before = OpenGlCapabilities::from_version(3, 2, false);
        let supported = OpenGlCapabilities::from_version(3, 3, false);

        assert!(!before.sampler_objects && supported.sampler_objects);
    }

    #[test]
    fn desktop_only_enable_flags_are_not_queried_on_opengl_es() {
        let capabilities = OpenGlCapabilities::from_version(3, 2, true);

        assert!(!capabilities.desktop_multisample && !capabilities.framebuffer_srgb);
    }
}
