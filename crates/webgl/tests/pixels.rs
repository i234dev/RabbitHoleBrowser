use rabbithole_webgl::{gl::*, Context, Error, MAX_DIMENSION, MAX_PIXELS, MAX_VERTEX_ATTRIBS};
const VERTEX: &str = "attribute vec2 aPosition; attribute vec3 aColor; varying vec3 vColor; void main(){ gl_Position=vec4(aPosition,0.0,1.0); vColor=aColor; }";
const FRAGMENT: &str =
    "precision mediump float; varying vec3 vColor; void main(){ gl_FragColor=vec4(vColor,1.0); }";
fn shader(c: &mut Context, kind: u32, src: &str) -> u32 {
    let id = c.create_shader(kind).unwrap();
    c.shader_source(id, src).unwrap();
    c.compile_shader(id).unwrap();
    assert!(
        c.shader_compile_status(id).unwrap(),
        "{}",
        c.shader_info_log(id).unwrap()
    );
    id
}
fn program(c: &mut Context, vs: &str, fs: &str) -> u32 {
    let v = shader(c, VERTEX_SHADER, vs);
    let f = shader(c, FRAGMENT_SHADER, fs);
    let p = c.create_program();
    c.attach_shader(p, v).unwrap();
    c.attach_shader(p, f).unwrap();
    c.link_program(p).unwrap();
    assert!(
        c.program_link_status(p).unwrap(),
        "{}",
        c.program_info_log(p).unwrap()
    );
    c.use_program(p).unwrap();
    p
}
fn attribute(c: &mut Context, p: u32, name: &str, size: u32, data: &[f32]) -> u32 {
    let id = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, id).unwrap();
    let bytes: Vec<_> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
    let at = c.get_attrib_location(p, name).unwrap();
    assert!(at >= 0);
    c.vertex_attrib_pointer(at as u32, size, FLOAT, false, 0, 0)
        .unwrap();
    c.enable_vertex_attrib_array(at as u32, true).unwrap();
    id
}
fn triangle() -> (Context, u32) {
    let mut c = Context::new(16, 16).unwrap();
    let p = program(&mut c, VERTEX, FRAGMENT);
    attribute(&mut c, p, "aPosition", 2, &[-1., -1., 1., -1., -1., 1.]);
    attribute(
        &mut c,
        p,
        "aColor",
        3,
        &[1., 0., 0., 0., 1., 0., 0., 0., 1.],
    );
    (c, p)
}
fn pixel(c: &Context, x: i32, y: i32) -> [u8; 4] {
    c.read_pixels(x, y, 1, 1).unwrap().try_into().unwrap()
}
#[test]
fn colored_triangle_produces_interpolated_pixels() {
    let (mut c, _) = triangle();
    c.bind_buffer(ARRAY_BUFFER, 0).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 4, 4), [112, 72, 72, 255]);
    assert_eq!(pixel(&c, 15, 15), [0; 4]);
    assert_eq!(pixel(&c, 8, 7), [0; 4]);
    assert!(
        c.framebuffer_rgba()
            .chunks_exact(4)
            .filter(|p| p[3] == 255)
            .count()
            > 90
    );
}
#[test]
fn clear_scissor_masks_orientation_and_resize() {
    let mut c = Context::new(4, 4).unwrap();
    c.clear_color([1., 0., 0., 1.]);
    c.scissor(0, 0, 2, 1).unwrap();
    c.enable_scissor(true);
    c.viewport(3, 3, 1, 1).unwrap();
    c.clear(COLOR_BUFFER_BIT).unwrap();
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 0, 3), [0; 4]);
    assert_eq!(&c.framebuffer_rgba()[..4], &[0; 4]);
    assert_eq!(&c.framebuffer_rgba()[48..52], &[255, 0, 0, 255]);
    assert_eq!(
        c.read_pixels(-1, -1, 2, 2).unwrap(),
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 255]
    );
    c.color_mask([false, true, false, true]);
    c.clear_color([0., 1., 1., 0.5]);
    c.clear(COLOR_BUFFER_BIT).unwrap();
    assert_eq!(pixel(&c, 0, 0), [255, 255, 0, 128]);
    c.resize(8, 8).unwrap();
    assert_eq!(c.viewport_rect(), [3, 3, 1, 1]);
    assert_eq!(c.scissor_rect(), [0, 0, 2, 1]);
    assert!(c.framebuffer_rgba().iter().all(|v| *v == 0));
}
#[test]
fn indexed_quad_covers_shared_edges_without_holes() {
    let mut c = Context::new(8, 8).unwrap();
    let p = program(
        &mut c,
        "attribute vec2 p; void main(){gl_Position=vec4(p,0.0,1.0);}",
        "precision mediump float; void main(){gl_FragColor=vec4(1.0);}",
    );
    attribute(&mut c, p, "p", 2, &[-1., -1., 1., -1., 1., 1., -1., 1.]);
    let b = c.create_buffer();
    c.bind_buffer(ELEMENT_ARRAY_BUFFER, b).unwrap();
    let bytes: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    c.buffer_data(ELEMENT_ARRAY_BUFFER, &bytes).unwrap();
    c.draw_elements(TRIANGLES, 6, UNSIGNED_SHORT, 0).unwrap();
    assert!(c.framebuffer_rgba().iter().all(|v| *v == 255));
}
#[test]
fn perspective_varyings_are_not_affine() {
    let mut c = Context::new(16, 16).unwrap();
    let p=program(&mut c,"attribute vec4 p; attribute vec3 color; varying vec3 vColor; void main(){gl_Position=p;vColor=color;}",FRAGMENT);
    attribute(
        &mut c,
        p,
        "p",
        4,
        &[-1., -1., 0., 1., 2., -2., 0., 2., -4., 4., 0., 4.],
    );
    attribute(&mut c, p, "color", 3, &[1., 0., 0., 0., 1., 0., 0., 0., 1.]);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 4, 4), [172, 55, 28, 255]);
}
#[test]
fn clipping_rejects_far_plane_and_keeps_visible_polygon() {
    let mut c = Context::new(16, 16).unwrap();
    let p = program(
        &mut c,
        "attribute vec3 p; void main(){gl_Position=vec4(p,1.0);}",
        "precision mediump float; void main(){gl_FragColor=vec4(1.0,0.0,0.0,1.0);}",
    );
    attribute(&mut c, p, "p", 3, &[-1., -1., 2., 1., -1., 2., -1., 1., 2.]);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert!(c.framebuffer_rgba().iter().all(|v| *v == 0));
    attribute(
        &mut c,
        p,
        "p",
        3,
        &[-2., -0.5, 0., 0.5, -0.5, 0., 0.5, 0.5, 0.],
    );
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 6), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 15, 15), [0; 4]);
}
#[test]
fn uniforms_matrix_and_depth_are_real() {
    let mut c = Context::new(8, 8).unwrap();
    let p=program(&mut c,"attribute vec2 p; uniform mat4 matrix; uniform float z; void main(){gl_Position=matrix*vec4(p,z,1.0);}","precision mediump float; uniform vec4 color; void main(){gl_FragColor=color;}");
    attribute(&mut c, p, "p", 2, &[-1., -1., 1., -1., -1., 1.]);
    let mat = c.get_uniform_location(p, "matrix").unwrap().unwrap();
    let z = c.get_uniform_location(p, "z").unwrap().unwrap();
    let color = c.get_uniform_location(p, "color").unwrap().unwrap();
    c.uniform_matrix4fv(
        mat,
        false,
        &[
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ],
    )
    .unwrap();
    c.uniform_f32(z, &[-0.5]).unwrap();
    c.uniform_f32(color, &[1., 0., 0., 1.]).unwrap();
    c.enable_depth_test(true);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 1, 1), [255, 0, 0, 255]);
    c.uniform_f32(z, &[0.5]).unwrap();
    c.uniform_f32(color, &[0., 0., 1., 1.]).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 1, 1), [255, 0, 0, 255]);
    c.depth_mask(false);
    c.clear(DEPTH_BUFFER_BIT).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 1, 1), [255, 0, 0, 255]);
    c.enable_depth_test(false);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 1, 1), [0, 0, 255, 255]);
    assert_eq!(
        c.uniform_matrix4fv(mat, true, &[0.; 16]),
        Err(Error::InvalidValue)
    );
    assert_eq!(
        c.uniform_f32(color, &[1., 2.]),
        Err(Error::InvalidOperation)
    );
}
#[test]
fn invalid_shaders_and_link_mismatches_fail_with_logs() {
    let mut c = Context::new(4, 4).unwrap();
    let s = c.create_shader(VERTEX_SHADER).unwrap();
    for src in [
        "void main(){gl_Position=vec4(1.0)",
        "void main(){gl_Position=missing;}",
        "void main(){for(int i=0;i<2;i++){} gl_Position=vec4(1.0);}",
        "void main(){gl_Position=texture2D(sampler,vec2(0.0));}",
        "attribute vec2 p; void main(){p=vec2(1.0);gl_Position=vec4(1.0);}",
        "void main(){gl_Position=vec3(1.0);}",
        "/*",
    ] {
        c.shader_source(s, src).unwrap();
        c.compile_shader(s).unwrap();
        assert!(!c.shader_compile_status(s).unwrap(), "accepted {src}");
        assert!(!c.shader_info_log(s).unwrap().is_empty());
    }
    let v = shader(
        &mut c,
        VERTEX_SHADER,
        "varying vec3 v; void main(){gl_Position=vec4(0.0);v=vec3(1.0);}",
    );
    let f = shader(
        &mut c,
        FRAGMENT_SHADER,
        "precision mediump float; varying vec4 v;void main(){gl_FragColor=v;}",
    );
    let p = c.create_program();
    c.attach_shader(p, v).unwrap();
    c.attach_shader(p, f).unwrap();
    c.link_program(p).unwrap();
    assert!(!c.program_link_status(p).unwrap());
    assert!(c.program_info_log(p).unwrap().contains("varying"));
    assert_eq!(c.use_program(p), Err(Error::InvalidOperation));
}
#[test]
fn bounds_errors_preserve_framebuffer_and_never_panic() {
    let (mut c, p) = triangle();
    let before = c.framebuffer_rgba().to_vec();
    assert_eq!(c.draw_arrays(TRIANGLES, 0, 4), Err(Error::InvalidOperation));
    assert_eq!(c.framebuffer_rgba(), before);
    assert_eq!(
        c.vertex_attrib_pointer(MAX_VERTEX_ATTRIBS, 2, FLOAT, false, 0, 0),
        Err(Error::InvalidValue)
    );
    assert_eq!(
        c.vertex_attrib_pointer(0, 2, FLOAT, false, 3, 0),
        Err(Error::InvalidValue)
    );
    assert_eq!(
        c.vertex_attrib_pointer(0, 2, FLOAT, false, 256, 0),
        Err(Error::InvalidValue)
    );
    assert_eq!(
        c.enable_vertex_attrib_array(u32::MAX, true),
        Err(Error::InvalidValue)
    );
    assert_eq!(
        c.buffer_data_size(ARRAY_BUFFER, usize::MAX),
        Err(Error::OutOfMemory)
    );
    assert_eq!(
        c.draw_arrays(TRIANGLES, u32::MAX, 3),
        Err(Error::InvalidValue)
    );
    let b = c.create_buffer();
    c.bind_buffer(ELEMENT_ARRAY_BUFFER, b).unwrap();
    c.buffer_data(ELEMENT_ARRAY_BUFFER, &[0, 1, 254]).unwrap();
    assert_eq!(
        c.draw_elements(TRIANGLES, 3, UNSIGNED_BYTE, 0),
        Err(Error::InvalidOperation)
    );
    assert_eq!(
        c.draw_elements(TRIANGLES, 3, UNSIGNED_SHORT, 1),
        Err(Error::InvalidOperation)
    );
    assert_eq!(c.framebuffer_rgba(), before);
    assert_eq!(c.resize(MAX_DIMENSION + 1, 1), Err(Error::OutOfMemory));
    assert_eq!(c.dimensions(), (16, 16));
    assert!(Context::new(MAX_DIMENSION, MAX_DIMENSION).is_err());
    assert_eq!(MAX_PIXELS, 4_194_304);
    assert_eq!(c.get_attrib_location(p, "absent"), Ok(-1));
}
#[test]
fn deleted_resources_are_not_reused_or_silently_drawn() {
    let (mut c, p) = triangle();
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    c.buffer_data(ARRAY_BUFFER, &[0; 24]).unwrap();
    let at = c.get_attrib_location(p, "aPosition").unwrap() as u32;
    c.vertex_attrib_pointer(at, 2, FLOAT, false, 0, 0).unwrap();
    c.delete_buffer(b).unwrap();
    assert_eq!(c.draw_arrays(TRIANGLES, 0, 3), Err(Error::InvalidOperation));
    let next = c.create_buffer();
    assert_ne!(next, b);
    assert_eq!(c.bind_buffer(ARRAY_BUFFER, b), Err(Error::InvalidOperation));
    c.delete_program(p).unwrap();
    assert!(c.program_link_status(p).unwrap());
    c.use_program(0).unwrap();
    assert_eq!(c.use_program(p), Err(Error::InvalidOperation));
}
#[test]
fn delete_attached_shader_defers_release_until_program_is_gone() {
    let mut c = Context::new(2, 2).unwrap();
    let v = shader(
        &mut c,
        VERTEX_SHADER,
        "void main(){gl_Position=vec4(0.0,0.0,0.0,1.0);}",
    );
    let f = shader(
        &mut c,
        FRAGMENT_SHADER,
        "precision mediump float;void main(){gl_FragColor=vec4(1.0);}",
    );
    let p = c.create_program();
    c.attach_shader(p, v).unwrap();
    c.attach_shader(p, f).unwrap();
    c.delete_shader(v).unwrap();
    c.delete_shader(f).unwrap();
    c.link_program(p).unwrap();
    assert!(c.program_link_status(p).unwrap());
    c.delete_program(p).unwrap();
    assert!(c.shader_compile_status(v).is_err());
}
#[test]
fn normalized_integer_attributes_and_constant_defaults() {
    let mut c = Context::new(8, 8).unwrap();
    let p = program(&mut c, VERTEX, FRAGMENT);
    attribute(&mut c, p, "aPosition", 2, &[-1., -1., 1., -1., -1., 1.]);
    let a = c.get_attrib_location(p, "aColor").unwrap() as u32;
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    c.buffer_data(ARRAY_BUFFER, &[128, 64, 255, 128, 64, 255, 128, 64, 255])
        .unwrap();
    c.vertex_attrib_pointer(a, 3, UNSIGNED_BYTE, true, 0, 0)
        .unwrap();
    c.enable_vertex_attrib_array(a, true).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 1, 1), [128, 64, 255, 255]);
    c.enable_vertex_attrib_array(a, false).unwrap();
    c.vertex_attrib(a, [0., 1., 0., 1.]).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 1, 1), [0, 255, 0, 255]);
}

#[test]
fn linked_reflection_uniform_values_and_relink_locations() {
    let mut c = Context::new(2, 2).unwrap();
    let p = program(
        &mut c,
        "attribute vec2 p; uniform mat4 matrix; void main(){gl_Position=matrix*vec4(p,0.0,1.0);}",
        "precision mediump float; uniform vec4 color; void main(){gl_FragColor=color;}",
    );
    let a = c.active_attributes(p).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].name, "p");
    assert_eq!(a[0].kind, FLOAT_VEC2);
    let u = c.active_uniforms(p).unwrap();
    assert_eq!(u.len(), 2);
    let loc = c.get_uniform_location(p, "color").unwrap().unwrap();
    assert_eq!(c.get_uniform(p, loc).unwrap(), vec![0.; 4]);
    c.uniform_f32(loc, &[0.25, 0.5, 0.75, 1.]).unwrap();
    assert_eq!(c.get_uniform(p, loc).unwrap(), vec![0.25, 0.5, 0.75, 1.]);
    c.link_program(p).unwrap();
    assert_eq!(c.uniform_f32(loc, &[0.; 4]), Err(Error::InvalidOperation));
    let new = c.get_uniform_location(p, "color").unwrap().unwrap();
    assert_ne!(loc, new);
    assert_eq!(c.get_uniform(p, new).unwrap(), vec![0.; 4]);
}
#[test]
fn fragment_precision_and_reserved_names_are_checked() {
    let mut c = Context::new(1, 1).unwrap();
    let f = c.create_shader(FRAGMENT_SHADER).unwrap();
    for src in [
        "void main(){gl_FragColor=vec4(1.0);}",
        "precision mediump vec4;void main(){gl_FragColor=vec4(1.0);}",
        "precision mediump float;uniform float if;void main(){gl_FragColor=vec4(if);}",
    ] {
        c.shader_source(f, src).unwrap();
        c.compile_shader(f).unwrap();
        assert!(!c.shader_compile_status(f).unwrap());
    }
}
#[test]
fn strip_and_fan_draw_real_quads() {
    for mode in [TRIANGLE_STRIP, TRIANGLE_FAN] {
        let mut c = Context::new(4, 4).unwrap();
        let p = program(
            &mut c,
            "attribute vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}",
            "precision mediump float;void main(){gl_FragColor=vec4(1.0);}",
        );
        let data = if mode == TRIANGLE_STRIP {
            vec![-1., -1., 1., -1., -1., 1., 1., 1.]
        } else {
            vec![-1., -1., 1., -1., 1., 1., -1., 1.]
        };
        attribute(&mut c, p, "p", 2, &data);
        c.draw_arrays(mode, 0, 4).unwrap();
        assert!(c.framebuffer_rgba().iter().all(|v| *v == 255));
    }
}

#[test]
fn integer_literals_convert_in_constructors_not_float_arithmetic() {
    let mut c = Context::new(4, 4).unwrap();
    let p = program(
        &mut c,
        "attribute vec2 p; void main(){gl_Position=vec4(p,0,1);}",
        "precision mediump float;void main(){gl_FragColor=vec4(1);}",
    );
    attribute(&mut c, p, "p", 2, &[-1., -1., 1., -1., -1., 1.]);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 0), [255; 4]);
    let f = c.create_shader(FRAGMENT_SHADER).unwrap();
    for src in [
        "precision mediump float;void main(){float x=1;gl_FragColor=vec4(x);}",
        "precision mediump float;void main(){gl_FragColor=vec4(1/2);}",
        "precision mediump float;void main(){gl_FragColor=vec4(010);}",
    ] {
        c.shader_source(f, src).unwrap();
        c.compile_shader(f).unwrap();
        assert!(!c.shader_compile_status(f).unwrap(), "accepted {src}");
    }
}
#[test]
fn draw_work_budget_rejects_before_changing_pixels() {
    let mut c = Context::new(2048, 2048).unwrap();
    let p=program(&mut c,"attribute vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}","precision mediump float;void main(){gl_FragColor=(vec4(1.0)*0.5+vec4(0.1))*0.5+vec4(0.1);}");
    attribute(&mut c, p, "p", 2, &[-1., -1., 1., -1., -1., 1.]);
    assert_eq!(c.draw_arrays(TRIANGLES, 0, 3), Err(Error::OutOfMemory));
    assert!(c.framebuffer_rgba().iter().all(|v| *v == 0));
}
