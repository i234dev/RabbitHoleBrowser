use rabbithole_webgl::{gl::*, Context, Error, MAX_DIMENSION, MAX_TEXTURE_UNITS};
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
fn setup(fragment: &str) -> (Context, u32) {
    let mut c = Context::new(8, 8).unwrap();
    let v=shader(&mut c,VERTEX_SHADER,"attribute vec2 p;attribute vec2 uv;varying vec2 vUv;void main(){gl_Position=vec4(p,0.0,1.0);vUv=uv;}");
    let f = shader(&mut c, FRAGMENT_SHADER, fragment);
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
    for (name, data) in [
        ("p", vec![-1f32, -1., 1., -1., -1., 1., 1., 1.]),
        ("uv", vec![0f32, 0., 1., 0., 0., 1., 1., 1.]),
    ] {
        let a = c.get_attrib_location(p, name).unwrap();
        if a < 0 {
            continue;
        }
        let b = c.create_buffer();
        c.bind_buffer(ARRAY_BUFFER, b).unwrap();
        let bytes: Vec<_> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
        c.vertex_attrib_pointer(a as u32, 2, FLOAT, false, 0, 0)
            .unwrap();
        c.enable_vertex_attrib_array(a as u32, true).unwrap();
    }
    (c, p)
}
const FS:&str="precision mediump float;uniform sampler2D tex;varying vec2 vUv;void main(){gl_FragColor=texture2D(tex,vUv);}";
const TEXELS: [u8; 16] = [
    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
];
fn image(c: &mut Context, width: u32, height: u32, data: Option<&[u8]>) -> u32 {
    let id = c.create_texture();
    c.bind_texture(TEXTURE_2D, id).unwrap();
    c.tex_image_2d(
        TEXTURE_2D,
        0,
        RGBA,
        width,
        height,
        0,
        RGBA,
        UNSIGNED_BYTE,
        data,
    )
    .unwrap();
    for p in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
        c.tex_parameter_i(TEXTURE_2D, p, NEAREST as i32).unwrap();
    }
    for p in [TEXTURE_WRAP_S, TEXTURE_WRAP_T] {
        c.tex_parameter_i(TEXTURE_2D, p, CLAMP_TO_EDGE as i32)
            .unwrap();
    }
    id
}
fn draw(c: &mut Context) {
    c.draw_arrays(TRIANGLE_STRIP, 0, 4).unwrap();
}
fn pixel(c: &Context, x: i32, y: i32) -> [u8; 4] {
    c.read_pixels(x, y, 1, 1).unwrap().try_into().unwrap()
}
#[test]
fn rgba_texture_is_sampled_with_correct_orientation_and_sub_updates() {
    let (mut c, _) = setup(FS);
    image(&mut c, 2, 2, Some(&TEXELS));
    draw(&mut c);
    assert_eq!(pixel(&c, 1, 1), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 6, 1), [0, 255, 0, 255]);
    assert_eq!(pixel(&c, 1, 6), [0, 0, 255, 255]);
    assert_eq!(pixel(&c, 6, 6), [255; 4]);
    c.tex_sub_image_2d(
        TEXTURE_2D,
        0,
        0,
        1,
        1,
        1,
        RGBA,
        UNSIGNED_BYTE,
        &[0, 255, 255, 255],
    )
    .unwrap();
    draw(&mut c);
    assert_eq!(pixel(&c, 1, 6), [0, 255, 255, 255]);
    assert_eq!(
        c.tex_sub_image_2d(TEXTURE_2D, 0, 2, 0, 1, 1, RGBA, UNSIGNED_BYTE, &[1; 4]),
        Err(Error::InvalidValue)
    );
    draw(&mut c);
    assert_eq!(pixel(&c, 1, 6), [0, 255, 255, 255]);
}
#[test]
fn texture_units_sampler_reflection_and_deletion_are_real() {
    let (mut c, p) = setup(FS);
    let loc = c.get_uniform_location(p, "tex").unwrap().unwrap();
    assert_eq!(c.active_uniforms(p).unwrap()[0].kind, SAMPLER_2D);
    assert_eq!(c.get_uniform(p, loc).unwrap(), vec![0.]);
    c.active_texture(TEXTURE0 + 3).unwrap();
    let t = image(&mut c, 1, 1, Some(&[200, 100, 50, 255]));
    draw(&mut c);
    assert_eq!(pixel(&c, 0, 0), [0, 0, 0, 255]);
    c.uniform_i32(loc, &[3]).unwrap();
    assert_eq!(c.get_uniform(p, loc).unwrap(), vec![3.]);
    draw(&mut c);
    assert_eq!(pixel(&c, 0, 0), [200, 100, 50, 255]);
    c.delete_texture(t).unwrap();
    draw(&mut c);
    assert_eq!(pixel(&c, 0, 0), [0, 0, 0, 255]);
    assert_eq!(c.bind_texture(TEXTURE_2D, t), Err(Error::InvalidOperation));
    assert_eq!(c.uniform_f32(loc, &[0.]), Err(Error::InvalidOperation));
    assert_eq!(
        c.uniform_i32(loc, &[MAX_TEXTURE_UNITS as i32]),
        Err(Error::InvalidValue)
    );
}
#[test]
fn nearest_clamp_repeat_and_mirror_obey_normalized_coordinates() {
    let(mut c,p)=setup("precision mediump float;uniform sampler2D tex;uniform vec2 uv;void main(){gl_FragColor=texture2D(tex,uv);}");
    image(&mut c, 2, 2, Some(&TEXELS));
    let loc = c.get_uniform_location(p, "uv").unwrap().unwrap();
    for (mode, u, color) in [
        (CLAMP_TO_EDGE, 1.25, [0, 255, 0, 255]),
        (REPEAT, 1.25, [255, 0, 0, 255]),
        (MIRRORED_REPEAT, 1.25, [0, 255, 0, 255]),
        (CLAMP_TO_EDGE, -0.25, [255, 0, 0, 255]),
        (REPEAT, -0.25, [0, 255, 0, 255]),
        (MIRRORED_REPEAT, -0.25, [255, 0, 0, 255]),
    ] {
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_WRAP_S, mode as i32)
            .unwrap();
        c.uniform_f32(loc, &[u, 0.25]).unwrap();
        draw(&mut c);
        assert_eq!(pixel(&c, 3, 3), color);
    }
}
#[test]
fn npot_incomplete_texture_is_black_until_clamped() {
    let (mut c, _) = setup(FS);
    image(
        &mut c,
        3,
        1,
        Some(&[255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255]),
    );
    c.tex_parameter_i(TEXTURE_2D, TEXTURE_WRAP_S, REPEAT as i32)
        .unwrap();
    draw(&mut c);
    assert_eq!(pixel(&c, 1, 1), [0, 0, 0, 255]);
    c.tex_parameter_i(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE as i32)
        .unwrap();
    draw(&mut c);
    assert_eq!(pixel(&c, 1, 1), [255, 0, 0, 255]);
}
#[test]
fn unsupported_filters_fail_explicitly_and_leave_pixels_untouched() {
    let (mut c, _) = setup(FS);
    c.clear_color([0., 0., 1., 1.]);
    c.clear(COLOR_BUFFER_BIT).unwrap();
    let t = c.create_texture();
    c.bind_texture(TEXTURE_2D, t).unwrap();
    assert_eq!(
        c.texture_parameter(TEXTURE_2D, TEXTURE_MIN_FILTER),
        Ok(NEAREST_MIPMAP_LINEAR as i32)
    );
    assert_eq!(
        c.texture_parameter(TEXTURE_2D, TEXTURE_MAG_FILTER),
        Ok(LINEAR as i32)
    );
    c.tex_image_2d(
        TEXTURE_2D,
        0,
        RGBA,
        2,
        2,
        0,
        RGBA,
        UNSIGNED_BYTE,
        Some(&TEXELS),
    )
    .unwrap();
    assert_eq!(
        c.draw_arrays(TRIANGLE_STRIP, 0, 4),
        Err(Error::InvalidOperation)
    );
    assert_eq!(pixel(&c, 0, 0), [0, 0, 255, 255]);
    assert_eq!(
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_MIN_FILTER, LINEAR as i32),
        Err(Error::InvalidEnum)
    );
    c.tex_parameter_i(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST as i32)
        .unwrap();
    assert_eq!(
        c.draw_arrays(TRIANGLE_STRIP, 0, 4),
        Err(Error::InvalidOperation)
    );
    c.tex_parameter_i(TEXTURE_2D, TEXTURE_MAG_FILTER, NEAREST as i32)
        .unwrap();
    draw(&mut c);
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
}
#[test]
fn texture_limits_short_uploads_and_unbound_targets_are_checked() {
    let (mut c, _) = setup(FS);
    assert_eq!(
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST as i32),
        Err(Error::InvalidOperation)
    );
    let t = image(&mut c, 2, 2, Some(&TEXELS));
    assert_eq!(
        c.active_texture(TEXTURE0 + MAX_TEXTURE_UNITS),
        Err(Error::InvalidEnum)
    );
    assert_eq!(
        c.tex_image_2d(
            TEXTURE_2D,
            0,
            RGBA,
            2,
            2,
            0,
            RGBA,
            UNSIGNED_BYTE,
            Some(&[1; 4])
        ),
        Err(Error::InvalidOperation)
    );
    assert_eq!(c.texture_dimensions(t), Ok((2, 2)));
    assert_eq!(
        c.tex_image_2d(
            TEXTURE_2D,
            0,
            RGBA,
            MAX_DIMENSION + 1,
            1,
            0,
            RGBA,
            UNSIGNED_BYTE,
            None
        ),
        Err(Error::OutOfMemory)
    );
    assert_eq!(
        c.tex_image_2d(TEXTURE_2D, 1, RGBA, 1, 1, 0, RGBA, UNSIGNED_BYTE, None),
        Err(Error::InvalidOperation)
    );
    assert_eq!(
        c.tex_image_2d(TEXTURE_2D, 0, RGBA, 1, 1, 1, RGBA, UNSIGNED_BYTE, None),
        Err(Error::InvalidValue)
    );
    draw(&mut c);
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
    image(&mut c, 2, 2, None);
    draw(&mut c);
    assert_eq!(pixel(&c, 0, 0), [0; 4]);
}
#[test]
fn malformed_sampler_expressions_do_not_compile() {
    let mut c = Context::new(1, 1).unwrap();
    let f = c.create_shader(FRAGMENT_SHADER).unwrap();
    for src in ["precision mediump float;void main(){gl_FragColor=texture2D(1.0,vec2(0.0));}","precision mediump float;uniform sampler2D tex;void main(){gl_FragColor=texture2D(tex,vec3(0.0));}","precision mediump float;uniform sampler2D tex;void main(){gl_FragColor=vec4(tex);}","precision mediump float;uniform sampler2D tex;void main(){gl_FragColor=texture2D(-tex,vec2(0.0));}"]{c.shader_source(f,src).unwrap();c.compile_shader(f).unwrap();assert!(!c.shader_compile_status(f).unwrap(),"accepted {src}");}
    let v = c.create_shader(VERTEX_SHADER).unwrap();
    c.shader_source(
        v,
        "uniform sampler2D tex;void main(){gl_Position=texture2D(tex,vec2(0.0));}",
    )
    .unwrap();
    c.compile_shader(v).unwrap();
    assert!(!c.shader_compile_status(v).unwrap());
}
