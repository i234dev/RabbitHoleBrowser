use rabbithole_webgl::{gl::*, Context, Error};

fn context(version: u32, width: u32) -> Context {
    if version == 300 {
        Context::new_webgl2(width, 1).unwrap()
    } else {
        Context::new(width, 1).unwrap()
    }
}
fn fragment(version: u32, extra: &str, body: &str) -> String {
    if version == 300 {
        format!(
            "#version 300 es\nprecision mediump float;out vec4 pixel;{extra}void main(){{{body}}}"
        )
    } else {
        format!("precision mediump float;{extra}void main(){{{body}}}")
    }
}
fn output(version: u32) -> &'static str {
    if version == 300 {
        "pixel"
    } else {
        "gl_FragColor"
    }
}
fn compile(c: &mut Context, kind: u32, source: &str) -> u32 {
    let s = c.create_shader(kind).unwrap();
    c.shader_source(s, source).unwrap();
    c.compile_shader(s).unwrap();
    s
}
fn shader(c: &mut Context, kind: u32, source: &str) -> u32 {
    let s = compile(c, kind, source);
    assert!(
        c.shader_compile_status(s).unwrap(),
        "{source}: {}",
        c.shader_info_log(s).unwrap()
    );
    s
}
fn scene(c: &mut Context, version: u32, vs: Option<&str>, fs: &str) -> (u32, u32) {
    let default_vs = if version == 300 {
        "#version 300 es\nin vec2 position;void main(){gl_Position=vec4(position,0.0,1.0);}"
    } else {
        "attribute vec2 position;void main(){gl_Position=vec4(position,0.0,1.0);}"
    };
    let v = shader(c, VERTEX_SHADER, vs.unwrap_or(default_vs));
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
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    let bytes: Vec<_> = [-1f32, -1., 3., -1., -1., 3.]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
    let position = c.get_attrib_location(p, "position").unwrap() as u32;
    c.vertex_attrib_pointer(position, 2, FLOAT, false, 0, 0)
        .unwrap();
    c.enable_vertex_attrib_array(position, true).unwrap();
    (p, f)
}
fn draw(c: &mut Context, x: i32) -> [u8; 4] {
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    c.read_pixels(x, 0, 1, 1).unwrap().try_into().unwrap()
}
fn near(actual: [u8; 4], expected: [u8; 4]) {
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
        "actual={actual:?}, expected={expected:?}"
    );
}

#[test]
fn nested_helpers_take_inputs_by_value_and_execute_local_statements() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let fs = fragment(version,
            "float half(in float x){x=x*0.5;return x;}vec3 tint(vec3 color,float factor){vec3 local=color*half(factor);return local;}",
            &format!("float f=1.0;{}=vec4(tint(vec3(1.0,0.5,0.25),f),f);", output(version)));
        scene(&mut c, version, None, &fs);
        near(draw(&mut c, 0), [128, 64, 32, 255]);
    }
}

#[test]
fn called_helpers_capture_global_uniforms_not_the_callers_shadowed_values() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let fs = fragment(version,
            "uniform float gain;float readGain(){return gain;}float caller(float gain){float local=gain;gain=0.0;return local+readGain();}",
            &format!("{}=vec4(caller(0.25));", output(version)));
        let (p, _) = scene(&mut c, version, None, &fs);
        let gain = c
            .get_uniform_location(p, "gain")
            .unwrap()
            .expect("Uniform used only in a nested helper must be active");
        c.uniform_f32(gain, &[0.5]).unwrap();
        near(draw(&mut c, 0), [191; 4]);
        c.uniform_f32(gain, &[0.25]).unwrap();
        near(draw(&mut c, 0), [128; 4]);
    }
}

#[test]
fn global_references_bind_before_a_later_local_declaration_shadows_the_name() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let fs=fragment(version,"uniform float gain;float value(){float earlier=gain;float gain=0.0;return earlier+gain;}",&format!("{}=vec4(value());",output(version)));
        let (p, _) = scene(&mut c, version, None, &fs);
        let gain = c.get_uniform_location(p, "gain").unwrap().unwrap();
        c.uniform_f32(gain, &[0.5]).unwrap();
        near(draw(&mut c, 0), [128; 4]);
    }
}

#[test]
fn local_names_become_visible_only_after_their_initializers() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let fs = fragment(version,
            "uniform float gain;float shade(){return gain;}float value(){float gain=gain;float shade=shade();float sin=sin(0.0);return gain+shade+sin;}",
            &format!("{}=vec4(value());", output(version)));
        let (p, _) = scene(&mut c, version, None, &fs);
        let gain = c
            .get_uniform_location(p, "gain")
            .unwrap()
            .expect("initializer reads must keep the outer uniform active");
        c.uniform_f32(gain, &[0.25]).unwrap();
        near(draw(&mut c, 0), [128; 4]);
        c.uniform_f32(gain, &[0.5]).unwrap();
        near(draw(&mut c, 0), [255; 4]);
        assert!(reject(
            version,
            "float f(){float missing=missing;return missing;}",
            &format!("{}=vec4(1.0);", output(version))
        )
        .contains("unknown variable missing"));
        assert!(reject(
            version,
            "",
            &format!("float missing=missing;{}=vec4(1.0);", output(version))
        )
        .contains("unknown variable missing"));
    }
}

#[test]
fn unused_helpers_do_not_activate_uniforms_and_call_locals_do_not_leak() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let fs = fragment(version,
            "uniform float unused;float first(){return unused;}float uncalled(float unused){return first();}float bump(float x){float local=x;local=local+0.25;return local;}",
            &format!("{}=vec4(bump(0.0),bump(0.25),bump(0.5),1.0);", output(version)));
        let (p, _) = scene(&mut c, version, None, &fs);
        assert_eq!(c.get_uniform_location(p, "unused").unwrap(), None);
        near(draw(&mut c, 0), [64, 128, 191, 255]);
    }
}

#[test]
fn helper_globals_work_in_both_vertex_and_fragment_interfaces() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let vs = if version == 300 {
            "#version 300 es\nin vec2 position;out vec3 tint;vec2 readPosition(){return position;}vec3 color(){return normalize(vec3(0.0,3.0,4.0));}void main(){gl_Position=vec4(readPosition(),0.0,1.0);tint=color();}"
        } else {
            "attribute vec2 position;varying vec3 tint;vec2 readPosition(){return position;}vec3 color(){return normalize(vec3(0.0,3.0,4.0));}void main(){gl_Position=vec4(readPosition(),0.0,1.0);tint=color();}"
        };
        let extra = if version == 300 {
            "in vec3 tint;vec3 incoming(){return tint;}"
        } else {
            "varying vec3 tint;vec3 incoming(){return tint;}"
        };
        let fs = fragment(
            version,
            extra,
            &format!("{}=vec4(incoming(),1.0);", output(version)),
        );
        scene(&mut c, version, Some(vs), &fs);
        near(draw(&mut c, 0), [0, 153, 204, 255]);
    }
}

#[test]
fn global_fragment_coordinates_and_texture_uniforms_are_current_per_invocation() {
    for version in [100, 300] {
        let mut c = context(version, 2);
        let fs = fragment(
            version,
            "float coordinate(){return gl_FragCoord.x/2.0;}",
            &format!("{}=vec4(coordinate(),0.0,0.0,1.0);", output(version)),
        );
        scene(&mut c, version, None, &fs);
        near(draw(&mut c, 0), [64, 0, 0, 255]);
        near(draw(&mut c, 1), [191, 0, 0, 255]);
        let sample = if version == 300 {
            "texture"
        } else {
            "texture2D"
        };
        let fs = fragment(version, &format!("uniform sampler2D image;vec4 lookup(in vec2 uv){{return {sample}(image,uv);}}vec4 brighten(vec2 uv){{return mix(lookup(uv),vec4(1.0),0.5);}}"), &format!("{}=brighten(vec2(0.5));", output(version)));
        let (p, _) = scene(&mut c, version, None, &fs);
        // The sampler must be linked even though its only read is in a helper.
        assert!(c.get_uniform_location(p, "image").unwrap().is_some());
        let t = c.create_texture();
        c.bind_texture(TEXTURE_2D, t).unwrap();
        for name in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
            c.tex_parameter_i(TEXTURE_2D, name, NEAREST as i32).unwrap();
        }
        c.tex_image_2d(
            TEXTURE_2D,
            0,
            RGBA,
            1,
            1,
            0,
            RGBA,
            UNSIGNED_BYTE,
            Some(&[255, 0, 0, 255]),
        )
        .unwrap();
        near(draw(&mut c, 0), [255, 128, 128, 255]);
    }
}

#[test]
fn helpers_accept_precision_qualifiers_zero_arguments_and_local_shadowing() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let fs = fragment(version,
            "highp float half(void){return 0.5;}vec4 shade(in highp vec4 color){float sin=half();return color*sin;}",
            &format!("{}=shade(vec4(1.0));",output(version))).replace("void main()", "void main(void)");
        scene(&mut c, version, None, &fs);
        near(draw(&mut c, 0), [128; 4]);
    }
}

#[test]
fn sampler_reads_in_helpers_keep_framebuffer_feedback_validation_active() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let sampler = if version == 300 {
            "texture"
        } else {
            "texture2D"
        };
        let fs = fragment(
            version,
            &format!("uniform sampler2D image;vec4 lookup(vec2 uv){{return {sampler}(image,uv);}}"),
            &format!("{}=lookup(vec2(0.5));", output(version)),
        );
        let (p, _) = scene(&mut c, version, None, &fs);
        assert!(c.get_uniform_location(p, "image").unwrap().is_some());
        c.clear_color([0., 0., 1., 1.]);
        c.clear(COLOR_BUFFER_BIT).unwrap();
        let t = c.create_texture();
        c.bind_texture(TEXTURE_2D, t).unwrap();
        for name in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
            c.tex_parameter_i(TEXTURE_2D, name, NEAREST as i32).unwrap();
        }
        c.tex_image_2d(
            TEXTURE_2D,
            0,
            RGBA,
            1,
            1,
            0,
            RGBA,
            UNSIGNED_BYTE,
            Some(&[0, 255, 0, 255]),
        )
        .unwrap();
        let f = c.create_framebuffer();
        c.bind_framebuffer(FRAMEBUFFER, f).unwrap();
        c.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, t, 0)
            .unwrap();
        assert_eq!(c.draw_arrays(TRIANGLES, 0, 3), Err(Error::InvalidOperation));
        assert_eq!(c.read_pixels(0, 0, 1, 1).unwrap(), [0, 255, 0, 255]);
        c.bind_framebuffer(FRAMEBUFFER, 0).unwrap();
        assert_eq!(c.read_pixels(0, 0, 1, 1).unwrap(), [0, 0, 255, 255]);
    }
}

fn reject(version: u32, extra: &str, body: &str) -> String {
    let mut c = context(version, 1);
    let source = fragment(version, extra, body);
    let s = compile(&mut c, FRAGMENT_SHADER, &source);
    assert!(
        !c.shader_compile_status(s).unwrap(),
        "accepted unsupported shader: {source}"
    );
    let log = c.shader_info_log(s).unwrap().to_string();
    assert!(!log.is_empty());
    log
}

#[test]
fn helper_types_arity_and_return_contract_are_checked_before_execution() {
    for version in [100, 300] {
        for expression in ["f()", "f(0.0,1.0)", "f(vec2(0.0))", "f(1)"] {
            reject(
                version,
                "float f(float x){return x;}",
                &format!("{}=vec4({expression});", output(version)),
            );
        }
        for helper in [
            "float f(float x){return vec2(x);}",
            "float f(float x){return 1;}",
            "float f(float x){float y=x;}",
            "float f(float x){return x;x=x+1.0;}",
            "float f(out float x){return x;}",
            "float f(inout float x){return x;}",
            "float f(const in float x){return x;}",
            "float f(sampler2D image){return 1.0;}",
            "float f(mat4 transform){return 1.0;}",
            "float f(float x,float x){return x;}",
            "float f(float x){float x=1.0;return x;}",
            "float f(float x){float sin=0.0;return sin(x);}",
        ] {
            reject(version, helper, &format!("{}=vec4(1.0);", output(version)));
        }
    }
}

#[test]
fn recursion_forward_calls_overloads_and_ignored_code_are_rejected() {
    for version in [100, 300] {
        for helper in [
            "float f(float x){return f(x);}",
            "float f(float x){return g(x);}float g(float x){return f(x);}",
            "float f(float x);float f(float x){return x;}",
            "float f(float x){return x;}vec2 f(vec2 x){return x;}",
            "float f(float x){return x;}float f(float x){return x;}",
            "float sin(float x){return x;}",
            "float texture(float x){return x;}",
            "float f(float x){return x;}uniform float f;",
            "uniform float f;float f(float x){return x;}",
            "void f(){return;}",
            "float f(float x){if(x){return x;}return 0.0;}",
        ] {
            reject(version, helper, &format!("{}=vec4(1.0);", output(version)));
        }
        let mut c = context(version, 1);
        let source = fragment(version, "", &format!("{}=vec4(1.0);", output(version)))
            + "float after(){return 0.0;}";
        let s = compile(&mut c, FRAGMENT_SHADER, &source);
        assert!(!c.shader_compile_status(s).unwrap());
        reject(
            version,
            "float f(float x){return x;}",
            &format!("float f=0.0;{}=vec4(f(1.0));", output(version)),
        );
    }
}

#[test]
fn helpers_reject_global_writes_and_mutable_global_reads() {
    for version in [100, 300] {
        let out = output(version);
        assert!(reject(
            version,
            &format!("float f(float x){{{out}=vec4(x);return x;}}"),
            &format!("{out}=vec4(1.0);")
        )
        .contains("write global"));
        assert!(reject(
            version,
            "uniform float gain;float f(float x){gain=x;return x;}",
            &format!("{out}=vec4(1.0);")
        )
        .contains("write global"));
        assert!(reject(
            version,
            &format!("vec4 f(){{return {out};}}"),
            &format!("{out}=vec4(1.0);")
        )
        .contains("mutable global"));
    }
}

#[test]
fn function_graph_and_call_site_costs_are_bounded_without_ast_expansion() {
    for version in [100, 300] {
        let mut chain = String::from("float f0(float x){return x;}");
        for i in 1..16 {
            chain += &format!("float f{i}(float x){{return f{}(x);}}", i - 1);
        }
        let mut c = context(version, 1);
        let source = fragment(
            version,
            &chain,
            &format!("{}=vec4(f15(0.5));", output(version)),
        );
        scene(&mut c, version, None, &source);
        near(draw(&mut c, 0), [128; 4]);
        let overflow = chain + "float f16(float x){return x;}";
        assert!(reject(
            version,
            &overflow,
            &format!("{}=vec4(1.0);", output(version))
        )
        .contains("too many user functions"));
        let mut expanding = String::from("float f0(float x){return x+x;}");
        for i in 1..10 {
            expanding += &format!("float f{i}(float x){{return f{}(x)+f{}(x);}}", i - 1, i - 1);
        }
        assert!(reject(
            version,
            &expanding,
            &format!("{}=vec4(1.0);", output(version))
        )
        .contains("budget"));
        assert!(reject(
            version,
            "float f(float x){return x;}",
            &format!(
                "float x=0.5;{}{}=vec4(x);",
                "x=f(x);".repeat(100),
                output(version)
            )
        )
        .contains("budget"));
        reject(version,"float f(float a,float b,float c,float d,float e,float f,float g,float h,float i){return a;}",&format!("{}=vec4(1.0);",output(version)));
    }
}

#[test]
fn a_failed_function_compile_cannot_leave_partial_shader_execution() {
    for version in [100, 300] {
        let mut c = context(version, 1);
        let out = output(version);
        let valid = fragment(
            version,
            "vec4 shade(vec4 color){return color;}",
            &format!("{out}=shade(vec4(1.0,0.0,0.0,1.0));"),
        );
        let (p, s) = scene(&mut c, version, None, &valid);
        assert_eq!(draw(&mut c, 0), [255, 0, 0, 255]);
        let invalid = fragment(
            version,
            "float f(float x){return f(x);}",
            &format!("{out}=vec4(f(1.0));"),
        );
        c.shader_source(s, &invalid).unwrap();
        c.compile_shader(s).unwrap();
        assert!(!c.shader_compile_status(s).unwrap());
        c.link_program(p).unwrap();
        assert!(!c.program_link_status(p).unwrap());
        assert_eq!(c.draw_arrays(TRIANGLES, 0, 3), Err(Error::InvalidOperation));
        assert_eq!(c.read_pixels(0, 0, 1, 1).unwrap(), [255, 0, 0, 255]);
        let valid = valid.replace("1.0,0.0,0.0,1.0", "0.0,1.0,0.0,1.0");
        c.shader_source(s, &valid).unwrap();
        c.compile_shader(s).unwrap();
        assert!(c.shader_compile_status(s).unwrap());
        c.link_program(p).unwrap();
        assert!(c.program_link_status(p).unwrap());
        assert_eq!(draw(&mut c, 0), [0, 255, 0, 255]);
    }
}
