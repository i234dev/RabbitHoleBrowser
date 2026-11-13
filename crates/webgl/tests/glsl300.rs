//! GLSL ES 3.00 backend fixtures; these do not expose a WebGL2 JS context.
use rabbithole_webgl::{gl::*, Context, Error};
const VS300:&str="#version 300 es\nlayout(location=3) in vec2 position;in vec3 color;out vec3 vColor;void main(){gl_Position=vec4(position,0.0,1.0);vColor=color;}";
const FS300:&str="#version 300 es\nprecision mediump float;in vec3 vColor;layout(location=0) out vec4 fragment;void main(){fragment=vec4(vColor,1.0);}";
const VS100:&str="attribute vec2 position;attribute vec3 color;varying vec3 vColor;void main(){gl_Position=vec4(position,0.0,1.0);vColor=color;}";
const FS100: &str =
    "precision mediump float;varying vec3 vColor;void main(){gl_FragColor=vec4(vColor,1.0);}";
fn shader(c: &mut Context, kind: u32, src: &str) -> u32 {
    let s = c.create_shader(kind).unwrap();
    c.shader_source(s, src).unwrap();
    c.compile_shader(s).unwrap();
    assert!(
        c.shader_compile_status(s).unwrap(),
        "{}",
        c.shader_info_log(s).unwrap()
    );
    s
}
fn program(c: &mut Context, vs: &str, fs: &str) -> u32 {
    let v = shader(c, VERTEX_SHADER, vs);
    let f = shader(c, FRAGMENT_SHADER, fs);
    let p = c.create_program();
    c.attach_shader(p, v).unwrap();
    c.attach_shader(p, f).unwrap();
    c.bind_attrib_location(p, 1, "position").unwrap();
    c.link_program(p).unwrap();
    assert!(
        c.program_link_status(p).unwrap(),
        "{}",
        c.program_info_log(p).unwrap()
    );
    c.use_program(p).unwrap();
    p
}
fn attribute(c: &mut Context, p: u32, name: &str, size: u32, data: &[f32]) {
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    let data: Vec<_> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    c.buffer_data(ARRAY_BUFFER, &data).unwrap();
    let a = c.get_attrib_location(p, name).unwrap() as u32;
    c.vertex_attrib_pointer(a, size, FLOAT, false, 0, 0)
        .unwrap();
    c.enable_vertex_attrib_array(a, true).unwrap();
}
fn pixel(c: &Context, x: i32, y: i32) -> [u8; 4] {
    c.read_pixels(x, y, 1, 1).unwrap().try_into().unwrap()
}
#[test]
fn backend300_triangle_and_explicit_locations_render_actual_pixels() {
    let mut c = Context::new_with_glsl_version(16, 16, 300).unwrap();
    let p = program(&mut c, VS300, FS300);
    assert_eq!(c.get_attrib_location(p, "position"), Ok(3));
    attribute(&mut c, p, "position", 2, &[-1., -1., 1., -1., -1., 1.]);
    attribute(&mut c, p, "color", 3, &[1., 0., 0., 0., 1., 0., 0., 0., 1.]);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 4, 4), [112, 72, 72, 255]);
    assert_eq!(pixel(&c, 15, 15), [0; 4]);
}
#[test]
fn backend300_accepts100_with_100_semantics() {
    let mut c = Context::new_with_glsl_version(16, 16, 300).unwrap();
    let p = program(&mut c, VS100, FS100);
    assert_eq!(c.get_attrib_location(p, "position"), Ok(1));
    attribute(&mut c, p, "position", 2, &[-1., -1., 1., -1., -1., 1.]);
    attribute(&mut c, p, "color", 3, &[1., 0., 0., 0., 1., 0., 0., 0., 1.]);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 4, 4), [112, 72, 72, 255]);
}
#[test]
fn existing_default_context_rejects300_without_claiming_webgl2() {
    let mut c = Context::new(2, 2).unwrap();
    let s = c.create_shader(VERTEX_SHADER).unwrap();
    c.shader_source(s, VS300).unwrap();
    c.compile_shader(s).unwrap();
    assert!(!c.shader_compile_status(s).unwrap());
    assert!(c.shader_info_log(s).unwrap().contains("100"));
    assert!(matches!(
        Context::new_with_glsl_version(1, 1, 310),
        Err(Error::InvalidValue)
    ));
}
#[test]
fn mixed_source_versions_fail_link_in_both_directions() {
    for (vs, fs) in [(VS100, FS300), (VS300, FS100)] {
        let mut c = Context::new_with_glsl_version(1, 1, 300).unwrap();
        let v = shader(&mut c, VERTEX_SHADER, vs);
        let f = shader(&mut c, FRAGMENT_SHADER, fs);
        let p = c.create_program();
        c.attach_shader(p, v).unwrap();
        c.attach_shader(p, f).unwrap();
        c.link_program(p).unwrap();
        assert!(!c.program_link_status(p).unwrap());
        assert!(c.program_info_log(p).unwrap().contains("versions differ"));
    }
}
#[test]
fn texture_function300_samples_rgba_through_named_output() {
    let mut c = Context::new_with_glsl_version(8, 8, 300).unwrap();
    let p=program(&mut c,"#version 300 es\nin vec2 position;in vec2 uv;out vec2 vUv;void main(){gl_Position=vec4(position,0.0,1.0);vUv=uv;}","#version 300 es\nprecision highp float;precision highp int;in vec2 vUv;uniform sampler2D tex;out vec4 outColor;void main(){outColor=texture(tex,vUv);}");
    attribute(
        &mut c,
        p,
        "position",
        2,
        &[-1., -1., 1., -1., -1., 1., 1., 1.],
    );
    attribute(&mut c, p, "uv", 2, &[0., 0., 1., 0., 0., 1., 1., 1.]);
    let t = c.create_texture();
    c.bind_texture(TEXTURE_2D, t).unwrap();
    let data = [
        255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
    ];
    c.tex_image_2d(
        TEXTURE_2D,
        0,
        RGBA,
        2,
        2,
        0,
        RGBA,
        UNSIGNED_BYTE,
        Some(&data),
    )
    .unwrap();
    for pname in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
        c.tex_parameter_i(TEXTURE_2D, pname, NEAREST as i32)
            .unwrap();
    }
    c.draw_arrays(TRIANGLE_STRIP, 0, 4).unwrap();
    assert_eq!(pixel(&c, 1, 1), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 6, 1), [0, 255, 0, 255]);
    assert_eq!(pixel(&c, 1, 6), [0, 0, 255, 255]);
    assert_eq!(pixel(&c, 6, 6), [255; 4]);
}
fn rejects(c: &mut Context, kind: u32, src: &str) {
    let s = c.create_shader(kind).unwrap();
    c.shader_source(s, src).unwrap();
    c.compile_shader(s).unwrap();
    assert!(!c.shader_compile_status(s).unwrap(), "accepted {src}");
    assert!(!c.shader_info_log(s).unwrap().is_empty());
}
#[test]
fn versioned_interfaces_and_legacy_builtin_names_are_checked() {
    let mut c = Context::new_with_glsl_version(1, 1, 300).unwrap();
    for src in [
        "#version 300\nvoid main(){gl_Position=vec4(1.0);}",
        "#version 310 es\nvoid main(){gl_Position=vec4(1.0);}",
        "#version 100\n#version 100\nvoid main(){gl_Position=vec4(1.0);}",
        "uniform float x;\n#version 300 es\nvoid main(){gl_Position=vec4(x);}",
        "#version 300 es\nattribute vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}",
        "#version 300 es\nvarying vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}",
        "in vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}",
    ] {
        rejects(&mut c, VERTEX_SHADER, src);
    }
    for src in ["#version 300 es\nprecision mediump float;out vec4 color;void main(){gl_FragColor=vec4(1.0);}","#version 300 es\nprecision mediump float;uniform sampler2D tex;out vec4 color;void main(){color=texture2D(tex,vec2(0.0));}","precision mediump float;uniform sampler2D tex;void main(){gl_FragColor=texture(tex,vec2(0.0));}"]{rejects(&mut c,FRAGMENT_SHADER,src);}
}
#[test]
fn mrt_and_unsupported_layouts_fail_explicitly() {
    let mut c = Context::new_with_glsl_version(1, 1, 300).unwrap();
    for src in ["#version 300 es\nprecision mediump float;out vec4 a;out vec4 b;void main(){a=vec4(1.0);b=vec4(0.0);}","#version 300 es\nprecision mediump float;layout(location=1) out vec4 a;void main(){a=vec4(1.0);}","#version 300 es\nprecision mediump float;out vec3 a;void main(){a=vec3(1.0);}","#version 300 es\nprecision mediump float;void main(){vec4 a=vec4(1.0);}","#version 300 es\nprecision mediump float;layout(location=0) in vec4 v;out vec4 a;void main(){a=v;}"]{rejects(&mut c,FRAGMENT_SHADER,src);}
    for src in ["#version 300 es\nlayout(location=16) in vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}","#version 300 es\nlayout(location=0.0) in vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}","#version 300 es\nlayout(location=0) out vec2 v;void main(){gl_Position=vec4(1.0);v=vec2(0.0);}"]{rejects(&mut c,VERTEX_SHADER,src);}
}
#[test]
fn aliased_active_locations_fail_link() {
    let mut c = Context::new_with_glsl_version(1, 1, 300).unwrap();
    let v=shader(&mut c,VERTEX_SHADER,"#version 300 es\nlayout(location=0) in vec2 p;layout(location=0) in vec2 q;void main(){gl_Position=vec4(p+q,0.0,1.0);}");
    let f = shader(
        &mut c,
        FRAGMENT_SHADER,
        "#version 300 es\nprecision mediump float;out vec4 a;void main(){a=vec4(1.0);}",
    );
    let p = c.create_program();
    c.attach_shader(p, v).unwrap();
    c.attach_shader(p, f).unwrap();
    c.link_program(p).unwrap();
    assert!(!c.program_link_status(p).unwrap());
    assert!(c.program_info_log(p).unwrap().contains("aliased"));
}
#[test]
fn token_limits_follow_backend_context_capability_and_prefixes_are_reserved() {
    let name = "a".repeat(257);
    let source = format!("attribute vec2 {name};void main(){{gl_Position=vec4({name},0.0,1.0);}}");
    let mut old = Context::new(1, 1).unwrap();
    rejects(&mut old, VERTEX_SHADER, &source);
    let mut newer = Context::new_with_glsl_version(1, 1, 300).unwrap();
    shader(&mut newer, VERTEX_SHADER, &source);
    let name = "a".repeat(1025);
    rejects(
        &mut newer,
        VERTEX_SHADER,
        &format!(
            "#version 300 es\nin vec2 {name};void main(){{gl_Position=vec4({name},0.0,1.0);}}"
        ),
    );
    rejects(&mut newer,VERTEX_SHADER,"#version 300 es\nin vec2 webgl_private;void main(){gl_Position=vec4(webgl_private,0.0,1.0);}");
}

#[test]
fn legacy_uniform_named_texture_keeps_variable_lookup_in_both_backend_modes() {
    for version in [100, 300] {
        let mut c = Context::new_with_glsl_version(4, 4, version).unwrap();
        let p=program(&mut c,"attribute vec2 position;varying vec2 uv;void main(){gl_Position=vec4(position,0.0,1.0);uv=position*0.5+0.5;}","precision mediump float;uniform sampler2D texture;varying vec2 uv;void main(){gl_FragColor=texture2D(texture,uv);}");
        attribute(
            &mut c,
            p,
            "position",
            2,
            &[-1., -1., 1., -1., -1., 1., 1., 1.],
        );
        let t = c.create_texture();
        c.bind_texture(TEXTURE_2D, t).unwrap();
        c.tex_image_2d(
            TEXTURE_2D,
            0,
            RGBA,
            1,
            1,
            0,
            RGBA,
            UNSIGNED_BYTE,
            Some(&[50, 100, 150, 200]),
        )
        .unwrap();
        for pname in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
            c.tex_parameter_i(TEXTURE_2D, pname, NEAREST as i32)
                .unwrap();
        }
        c.draw_arrays(TRIANGLE_STRIP, 0, 4).unwrap();
        assert_eq!(pixel(&c, 0, 0), [50, 100, 150, 200]);
    }
}
#[test]
fn local_builtin_shadowing_is_not_mistaken_for_a_function_call() {
    let mut c = Context::new_with_glsl_version(1, 1, 300).unwrap();
    shader(&mut c,FRAGMENT_SHADER,"#version 300 es\nprecision mediump float;out vec4 result;void main(){vec4 texture=vec4(1.0);result=texture;}");
    rejects(&mut c,FRAGMENT_SHADER,"#version 300 es\nprecision mediump float;uniform sampler2D tex;out vec4 result;void main(){vec4 texture=vec4(1.0);result=texture(tex,vec2(0.0));}");
    rejects(&mut c,FRAGMENT_SHADER,"#version 300 es\nprecision mediump float;uniform sampler2D texture;out vec4 result;void main(){result=vec4(1.0);}");
}
