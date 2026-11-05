//! Real frosted glass for what floats over scrolling content. The frame so far
//! is copied from under a rounded rectangle, blurred, and put back with its
//! brightness held in a band, so whatever scrolls behind can never compete with
//! the words on top.
//!
//! It is drawn with OpenGL in the middle of egui's frame, so it costs only what
//! the small region costs, and only on frames the app draws anyway. If the
//! driver will not build the shaders, [`available`] turns false and the card
//! falls back to a nearly solid fill.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use eframe::glow::{self, HasContext};
use egui::{Context, Id, Rect, Shape};

/// How far past the card the blur reads, in pixels, so that an edge blends what
/// is about to slide under it rather than smearing the last column.
const REACH: i32 = 32;
/// Downsampling steps. Each one about doubles the blur's radius.
const STEPS: usize = 3;
/// The brightest the blurred backdrop gets under the dark theme, and the
/// darkest under the light theme, in gamma-encoded luminance. With the glass
/// fill on top, these keep the secondary text at about 4.5:1 over a white cover.
const DARK_CEILING: f32 = 0.30;
const LIGHT_FLOOR: f32 = 0.84;
/// How much the blurred colours are saturated, the way frosted glass lets
/// colour through while it hides shapes.
const SATURATION: f32 = 1.35;

const BROKEN: u8 = 2;
const WORKS: u8 = 1;
/// Whether the shaders built. Shared by the window's one GL context.
static STATE: AtomicU8 = AtomicU8::new(0);

/// Whether a card can rely on the blur to keep its words readable.
pub fn available() -> bool {
    STATE.load(Ordering::Relaxed) != BROKEN
}

const VERTEX: &str = "#version 140
out vec2 uv;
void main() {
    uv = vec2(float(gl_VertexID & 1) * 2.0, float((gl_VertexID >> 1) & 1) * 2.0);
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}";

/// The dual Kawase blur's way down: a wide, cheap filter that halves the image.
const DOWN: &str = "#version 140
uniform sampler2D source;
uniform vec2 texel;
uniform vec2 area;
in vec2 uv;
out vec4 color;
vec4 at(vec2 place) {
    return texture(source, min(place, area - texel * 0.5));
}
void main() {
    vec2 place = uv * area;
    vec4 sum = at(place) * 4.0;
    sum += at(place - texel);
    sum += at(place + texel);
    sum += at(place + vec2(texel.x, -texel.y));
    sum += at(place + vec2(-texel.x, texel.y));
    color = sum / 8.0;
}";

/// And its way back up, which doubles it without showing the steps.
const UP: &str = "#version 140
uniform sampler2D source;
uniform vec2 texel;
uniform vec2 area;
in vec2 uv;
out vec4 color;
vec4 at(vec2 place) {
    return texture(source, min(place, area - texel * 0.5));
}
void main() {
    vec2 place = uv * area;
    vec4 sum = at(place + vec2(-texel.x * 2.0, 0.0));
    sum += at(place + vec2(-texel.x, texel.y)) * 2.0;
    sum += at(place + vec2(0.0, texel.y * 2.0));
    sum += at(place + vec2(texel.x, texel.y)) * 2.0;
    sum += at(place + vec2(texel.x * 2.0, 0.0));
    sum += at(place + vec2(texel.x, -texel.y)) * 2.0;
    sum += at(place + vec2(0.0, -texel.y * 2.0));
    sum += at(place + vec2(-texel.x, -texel.y)) * 2.0;
    color = sum / 12.0;
}";

/// Puts the blur on the card: rounded, saturated and held in its brightness band.
const FINISH: &str = "#version 140
uniform sampler2D source;
uniform vec4 window;
uniform vec2 size;
uniform float radius;
uniform float saturation;
uniform float ceiling;
uniform float floor_;
in vec2 uv;
out vec4 color;
void main() {
    vec3 rgb = texture(source, window.xy + uv * window.zw).rgb;
    float grey = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    rgb = clamp(mix(vec3(grey), rgb, saturation), 0.0, 1.0);
    grey = dot(rgb, vec3(0.2126, 0.7152, 0.0722));
    if (grey > ceiling) rgb *= ceiling / grey;
    if (grey < floor_) rgb = mix(rgb, vec3(1.0), (floor_ - grey) / max(1.0 - grey, 0.001));
    vec2 q = abs((uv - 0.5) * size) - size * 0.5 + radius;
    float edge = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    float inside = clamp(0.5 - edge, 0.0, 1.0);
    color = vec4(rgb * inside, inside);
}";

struct Program {
    program: glow::Program,
    source: Option<glow::UniformLocation>,
    texel: Option<glow::UniformLocation>,
    area: Option<glow::UniformLocation>,
    window: Option<glow::UniformLocation>,
    size: Option<glow::UniformLocation>,
    radius: Option<glow::UniformLocation>,
    saturation: Option<glow::UniformLocation>,
    ceiling: Option<glow::UniformLocation>,
    floor: Option<glow::UniformLocation>,
}

/// A step of the blur: a texture big enough for any card so far, of which the
/// card in hand uses a corner. Cards change size as they animate; the textures
/// stay, and only ever grow.
struct Level {
    texture: glow::Texture,
    capacity: [i32; 2],
}

/// The part of a level of `capacity` that a region of `size` uses at `step`.
fn used(size: [i32; 2], step: usize) -> [i32; 2] {
    [(size[0] >> step).max(1), (size[1] >> step).max(1)]
}

struct Blur {
    down: Program,
    up: Program,
    finish: Program,
    vao: glow::VertexArray,
    framebuffer: glow::Framebuffer,
    /// The copy of the frame, then each smaller step.
    levels: Vec<Level>,
}

#[derive(Clone, Default)]
struct Shared(Arc<Mutex<Option<Blur>>>);

unsafe fn compile(gl: &glow::Context, fragment: &str) -> Result<Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, text) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, text);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                return Err(gl.get_shader_info_log(shader));
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            return Err(gl.get_program_info_log(program));
        }
        let uniform = |name: &str| gl.get_uniform_location(program, name);
        Ok(Program {
            program,
            source: uniform("source"),
            texel: uniform("texel"),
            area: uniform("area"),
            window: uniform("window"),
            size: uniform("size"),
            radius: uniform("radius"),
            saturation: uniform("saturation"),
            ceiling: uniform("ceiling"),
            floor: uniform("floor_"),
        })
    }
}

unsafe fn build(gl: &glow::Context) -> Result<Blur, String> {
    unsafe {
        Ok(Blur {
            down: compile(gl, DOWN)?,
            up: compile(gl, UP)?,
            finish: compile(gl, FINISH)?,
            vao: gl.create_vertex_array()?,
            framebuffer: gl.create_framebuffer()?,
            levels: Vec::new(),
        })
    }
}

unsafe fn fit_levels(gl: &glow::Context, blur: &mut Blur, size: [i32; 2]) -> Result<(), String> {
    let held = blur.levels.first().map_or([0, 0], |level| level.capacity);
    if held[0] >= size[0] && held[1] >= size[1] {
        return Ok(());
    }
    // A little room ahead, so a card growing by a point does not come back here.
    let room = |side: i32, had: i32| ((side.max(had) + 255) / 256) * 256;
    let capacity = [room(size[0], held[0]), room(size[1], held[1])];
    unsafe {
        for level in blur.levels.drain(..) {
            gl.delete_texture(level.texture);
        }
        for step in 0..=STEPS {
            let level_size = used(capacity, step);
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                level_size[0],
                level_size[1],
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            blur.levels.push(Level {
                texture,
                capacity: level_size,
            });
        }
    }
    Ok(())
}

/// One pass of the blur from a level into another.
unsafe fn pass(
    gl: &glow::Context,
    blur: &Blur,
    program: &Program,
    size: [i32; 2],
    from: usize,
    to: usize,
) {
    let source = &blur.levels[from];
    let target = &blur.levels[to];
    let read = used(size, from);
    let write = used(size, to);
    unsafe {
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(target.texture),
            0,
        );
        gl.viewport(0, 0, write[0], write[1]);
        gl.use_program(Some(program.program));
        gl.bind_texture(glow::TEXTURE_2D, Some(source.texture));
        gl.uniform_1_i32(program.source.as_ref(), 0);
        gl.uniform_2_f32(
            program.texel.as_ref(),
            1.0 / source.capacity[0] as f32,
            1.0 / source.capacity[1] as f32,
        );
        gl.uniform_2_f32(
            program.area.as_ref(),
            read[0] as f32 / source.capacity[0] as f32,
            read[1] as f32 / source.capacity[1] as f32,
        );
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }
}

/// Copies, blurs and paints. Runs with egui's GL state, which egui restores
/// after the callback; the framebuffer is put back here.
unsafe fn draw(
    gl: &glow::Context,
    blur: &mut Blur,
    screen: [i32; 2],
    card: [i32; 4],
    radius: f32,
    dark: bool,
    back: Option<glow::Framebuffer>,
) -> Result<(), String> {
    let [left, bottom, width, height] = card;
    if width <= 0 || height <= 0 {
        return Ok(());
    }
    // Read a little past the card, as far as the window allows.
    let x0 = (left - REACH).max(0);
    let y0 = (bottom - REACH).max(0);
    let x1 = (left + width + REACH).min(screen[0]);
    let y1 = (bottom + height + REACH).min(screen[1]);
    let size = [x1 - x0, y1 - y0];
    if size[0] <= 0 || size[1] <= 0 {
        return Ok(());
    }
    unsafe {
        fit_levels(gl, blur, size)?;
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(blur.levels[0].texture));
        gl.copy_tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, x0, y0, size[0], size[1]);

        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::BLEND);
        gl.bind_vertex_array(Some(blur.vao));
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(blur.framebuffer));
        for step in 0..STEPS {
            pass(gl, blur, &blur.down, size, step, step + 1);
        }
        for step in (1..STEPS).rev() {
            pass(gl, blur, &blur.up, size, step + 1, step);
        }

        gl.bind_framebuffer(glow::FRAMEBUFFER, back);
        gl.enable(glow::SCISSOR_TEST);
        gl.enable(glow::BLEND);
        gl.viewport(left, bottom, width, height);
        let finish = &blur.finish;
        gl.use_program(Some(finish.program));
        gl.bind_texture(glow::TEXTURE_2D, Some(blur.levels[1].texture));
        gl.uniform_1_i32(finish.source.as_ref(), 0);
        let level = &blur.levels[1];
        let part = used(size, 1);
        let across = part[0] as f32 / level.capacity[0] as f32 / size[0] as f32;
        let down = part[1] as f32 / level.capacity[1] as f32 / size[1] as f32;
        gl.uniform_4_f32(
            finish.window.as_ref(),
            (left - x0) as f32 * across,
            (bottom - y0) as f32 * down,
            width as f32 * across,
            height as f32 * down,
        );
        gl.uniform_2_f32(finish.size.as_ref(), width as f32, height as f32);
        gl.uniform_1_f32(finish.radius.as_ref(), radius);
        gl.uniform_1_f32(finish.saturation.as_ref(), SATURATION);
        gl.uniform_1_f32(
            finish.ceiling.as_ref(),
            if dark { DARK_CEILING } else { 1.0 },
        );
        gl.uniform_1_f32(finish.floor.as_ref(), if dark { 0.0 } else { LIGHT_FLOOR });
        gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }
    Ok(())
}

/// Paints the frosted backdrop of a card at `rect`. Paint it before the card's
/// own fill: everything painted so far this frame shows through it, blurred.
pub fn backdrop(ctx: &Context, painter: &egui::Painter, rect: Rect, radius: f32, dark: bool) {
    if !available() {
        return;
    }
    let shared = ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Shared>(Id::new("kit-glass"))
            .clone()
    });
    let callback = eframe::egui_glow::CallbackFn::new(move |info, painter| {
        let gl = painter.gl();
        let Ok(mut slot) = shared.0.lock() else {
            return;
        };
        if slot.is_none() && available() {
            // SAFETY: egui calls this on the thread whose GL context is current.
            match unsafe { build(gl) } {
                Ok(blur) => {
                    *slot = Some(blur);
                    STATE.store(WORKS, Ordering::Relaxed);
                }
                Err(error) => {
                    log::warn!("Frosted glass is off: {error}");
                    STATE.store(BROKEN, Ordering::Relaxed);
                }
            }
        }
        let Some(blur) = slot.as_mut() else {
            return;
        };
        let viewport = info.viewport_in_pixels();
        // SAFETY: as above; egui restores the state changed here right after
        // the callback, and the framebuffer is put back before it returns.
        let drawn = unsafe {
            draw(
                gl,
                blur,
                [info.screen_size_px[0] as i32, info.screen_size_px[1] as i32],
                [
                    viewport.left_px,
                    viewport.from_bottom_px,
                    viewport.width_px,
                    viewport.height_px,
                ],
                radius * info.pixels_per_point,
                dark,
                painter.intermediate_fbo(),
            )
        };
        if let Err(error) = drawn {
            log::warn!("Frosted glass is off: {error}");
            STATE.store(BROKEN, Ordering::Relaxed);
        }
    });
    painter.add(Shape::Callback(egui::PaintCallback {
        rect,
        callback: Arc::new(callback),
    }));
}
