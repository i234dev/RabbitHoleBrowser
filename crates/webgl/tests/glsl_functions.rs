use rabbithole_webgl::{gl::*, Context};

fn context(version: u32) -> Context {
    if version == 300 {
        Context::new_webgl2(1, 1).unwrap()
    } else {
        Context::new(1, 1).unwrap()
    }
}
fn shader(c: &mut Context, kind: u32, source: &str) -> u32 {
    let s = c.create_shader(kind).unwrap();
    c.shader_source(s, source).unwrap();
    c.compile_shader(s).unwrap();
    assert!(
        c.shader_compile_status(s).unwrap(),
        "{source}: {}",
        c.shader_info_log(s).unwrap()
    );
    s
}
fn sources(version: u32, extra: &str, expression: &str) -> (String, String) {
    let vs = if version == 300 {
        "#version 300 es\nin vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}"
    } else {
        "attribute vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}"
    };
    let fs = if version == 300 {
        format!("#version 300 es\nprecision mediump float;out vec4 color;{extra}void main(){{color={expression};}}")
    } else {
        format!("precision mediump float;{extra}void main(){{gl_FragColor={expression};}}")
    };
    (vs.into(), fs)
}
fn scene(c: &mut Context, vs: &str, fs: &str) {
    let vs = shader(c, VERTEX_SHADER, vs);
    let fs = shader(c, FRAGMENT_SHADER, fs);
    let p = c.create_program();
    c.attach_shader(p, vs).unwrap();
    c.attach_shader(p, fs).unwrap();
    c.link_program(p).unwrap();
    assert!(
        c.program_link_status(p).unwrap(),
        "{}",
        c.program_info_log(p).unwrap()
    );
    c.use_program(p).unwrap();
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    let data: Vec<_> = [-1f32, -1., 3., -1., -1., 3.]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    c.buffer_data(ARRAY_BUFFER, &data).unwrap();
    let location = c.get_attrib_location(p, "p").unwrap() as u32;
    c.vertex_attrib_pointer(location, 2, FLOAT, false, 0, 0)
        .unwrap();
    c.enable_vertex_attrib_array(location, true).unwrap();
}
fn pixel(c: &mut Context) -> [u8; 4] {
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    c.read_pixels(0, 0, 1, 1).unwrap().try_into().unwrap()
}
fn render(version: u32, expression: &str) -> [u8; 4] {
    let mut c = context(version);
    let (vs, fs) = sources(version, "", expression);
    scene(&mut c, &vs, &fs);
    pixel(&mut c)
}
fn near(actual: [u8; 4], expected: [u8; 4], expression: &str) {
    assert!(
        actual.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= 1),
        "{expression}: got {actual:?}, expected {expected:?}"
    );
}

#[test]
fn trigonometric_exponential_and_rounding_functions_produce_real_pixels() {
    let cases = [
        (
            "vec4(sin(radians(90.0)),cos(0.0),tan(0.0),1.0)",
            [255, 255, 0, 255],
        ),
        (
            "vec4(degrees(asin(0.5))/90.0,degrees(acos(0.5))/90.0,degrees(atan(1.0))/90.0,1.0)",
            [85, 170, 128, 255],
        ),
        (
            "vec4(atan(1.0,-1.0)/radians(180.0),atan(-1.0,-1.0)/radians(-180.0),0.0,1.0)",
            [191, 191, 0, 255],
        ),
        (
            "vec4(exp(0.0),log(exp(0.5)),exp2(-1.0),1.0)",
            [255, 128, 128, 255],
        ),
        (
            "vec4(log2(2.0),sqrt(0.25),inversesqrt(4.0),1.0)",
            [255, 128, 128, 255],
        ),
        (
            "vec4(abs(-0.25),sign(-3.0)*0.25+0.5,sign(0.0),sign(7.0))",
            [64, 64, 0, 255],
        ),
        (
            "vec4(floor(0.75),ceil(0.25),fract(-0.25),1.0)",
            [0, 255, 191, 255],
        ),
        (
            "vec4(mod(-0.25,0.5),min(0.75,0.25),max(0.75,0.25),1.0)",
            [64, 64, 191, 255],
        ),
        (
            "vec4(pow(vec3(0.5,0.25,0.75),vec3(2.0)),1.0)",
            [64, 16, 143, 255],
        ),
        (
            "vec4(sin(vec3(0.0,radians(30.0),radians(90.0))),1.0)",
            [0, 128, 255, 255],
        ),
        (
            "vec4(atan(vec2(1.0,-1.0),vec2(-1.0,-1.0))/radians(vec2(180.0,-180.0)),0.0,1.0)",
            [191, 191, 0, 255],
        ),
        (
            "vec4(sin(float(0)),cos(float(0)),abs(float(-1)),1.0)",
            [0, 255, 255, 255],
        ),
    ];
    for version in [100, 300] {
        for (expression, expected) in cases {
            near(render(version, expression), expected, expression);
        }
    }
}

#[test]
fn scalar_vector_overloads_and_interpolation_obey_glsl_shapes() {
    let cases = [
        ("clamp(vec4(-0.5,0.25,2.0,1.0),0.0,1.0)", [0, 64, 255, 255]),
        (
            "clamp(vec4(-0.5,0.25,2.0,1.0),vec4(0.1,0.3,0.0,0.0),vec4(0.9,0.7,0.6,1.0))",
            [26, 77, 153, 255],
        ),
        (
            "mix(vec4(0.0),vec4(1.0),vec4(0.25,0.5,0.75,1.0))",
            [64, 128, 191, 255],
        ),
        ("mix(vec4(0.0),vec4(1.0),0.5)", [128, 128, 128, 128]),
        (
            "step(vec4(0.5),vec4(0.25,0.5,0.75,1.0))",
            [0, 255, 255, 255],
        ),
        ("step(0.5,vec4(0.25,0.5,0.75,1.0))", [0, 255, 255, 255]),
        (
            "smoothstep(0.0,1.0,vec4(0.0,0.25,0.5,1.0))",
            [0, 40, 128, 255],
        ),
        (
            "smoothstep(vec4(0.0),vec4(1.0),vec4(0.0,0.25,0.5,1.0))",
            [0, 40, 128, 255],
        ),
        ("min(vec4(0.25,0.75,0.5,1.0),0.5)", [64, 128, 128, 128]),
        ("max(vec4(0.25,0.75,0.5,1.0),0.5)", [128, 191, 128, 255]),
        ("mod(vec4(-0.25,0.75,1.0,0.5),0.5)", [64, 64, 0, 0]),
    ];
    for version in [100, 300] {
        for (expression, expected) in cases {
            near(render(version, expression), expected, expression);
        }
    }
}

#[test]
fn geometric_functions_return_scalar_or_vector_results_as_declared() {
    let cases = [
        ("vec4(length(vec2(0.3,0.4)),distance(vec2(0.0),vec2(0.3,0.4)),dot(vec2(0.5),vec2(0.5)),1.0)", [128,128,128,255]),
        ("vec4(normalize(vec3(0.0,3.0,4.0)),1.0)", [0,153,204,255]),
        ("vec4(cross(vec3(1,0,0),vec3(0,1,0)),1.0)", [0,0,255,255]),
        ("vec4(normalize(-2.0)*0.25+0.5,length(-0.5),distance(-0.25,0.25),dot(-0.5,-1.0))", [64,128,128,128]),
    ];
    for version in [100, 300] {
        for (expression, expected) in cases {
            near(render(version, expression), expected, expression);
        }
    }
}

#[test]
fn vertex_math_varyings_and_texture_arguments_share_the_evaluator() {
    for version in [100, 300] {
        let mut c = context(version);
        let (vs, fs) = if version == 300 {
            ("#version 300 es\nin vec2 p;out vec3 v;void main(){gl_Position=vec4(p+vec2(sin(0.0)),0.0,1.0);v=normalize(vec3(0.0,3.0,4.0));}",
             "#version 300 es\nprecision mediump float;in vec3 v;out vec4 color;void main(){color=vec4(v,1.0);}")
        } else {
            ("attribute vec2 p;varying vec3 v;void main(){gl_Position=vec4(p+vec2(sin(0.0)),0.0,1.0);v=normalize(vec3(0.0,3.0,4.0));}",
             "precision mediump float;varying vec3 v;void main(){gl_FragColor=vec4(v,1.0);}")
        };
        scene(&mut c, vs, fs);
        near(pixel(&mut c), [0, 153, 204, 255], "vertex normalize");
        let expression = if version == 300 {
            "mix(texture(tex,vec2(0.5)),vec4(1.0),0.5)"
        } else {
            "mix(texture2D(tex,vec2(0.5)),vec4(1.0),0.5)"
        };
        let (vs, fs) = sources(version, "uniform sampler2D tex;", expression);
        scene(&mut c, &vs, &fs);
        let texture = c.create_texture();
        c.bind_texture(TEXTURE_2D, texture).unwrap();
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
        near(pixel(&mut c), [255, 128, 128, 255], "mix sampled texture");
    }
}

fn rejects(version: u32, extra: &str, expression: &str) -> String {
    let mut c = context(version);
    let (_, source) = sources(version, extra, expression);
    let s = c.create_shader(FRAGMENT_SHADER).unwrap();
    c.shader_source(s, &source).unwrap();
    c.compile_shader(s).unwrap();
    assert!(
        !c.shader_compile_status(s).unwrap(),
        "accepted unsupported {source}"
    );
    let log = c.shader_info_log(s).unwrap().to_string();
    assert!(!log.is_empty());
    log
}

#[test]
fn math_overloads_reject_wrong_arity_type_and_implicit_integer_conversions() {
    for version in [100, 300] {
        for expression in [
            "vec4(sin())",
            "vec4(sin(0.0,1.0))",
            "vec4(length())",
            "vec4(mix(0.0,1.0))",
            "vec4(clamp(0.0,1.0,2.0,3.0))",
            "vec4(sin(1))",
            "vec4(min(1,1))",
            "vec4(normalize(mat4(1.0)))",
            "vec4(min(0.5,vec2(1.0)),0.0,1.0)",
            "vec4(max(vec2(0.0),vec3(1.0)),0.0,1.0)",
            "vec4(clamp(vec2(0.5),vec2(0.0),1.0),0.0,1.0)",
            "vec4(clamp(0.5,vec2(0.0),vec2(1.0)))",
            "vec4(mix(vec2(0.0),vec3(1.0),0.5),0.0,1.0)",
            "vec4(mix(vec2(0.0),vec2(1.0),vec3(0.5)),0.0,1.0)",
            "vec4(step(vec2(0.0),0.5))",
            "vec4(smoothstep(vec2(0.0),1.0,vec2(0.5)),0.0,1.0)",
            "vec4(pow(vec2(0.5),2.0),0.0,1.0)",
            "vec4(atan(vec2(0.5),1.0),0.0,1.0)",
            "vec4(cross(vec2(0.0),vec2(1.0)),1.0)",
            "vec4(dot(vec2(0.0),vec3(1.0)))",
            "vec4(unsupported(1.0))",
        ] {
            rejects(version, "", expression);
        }
        rejects(version, "uniform sampler2D tex;", "vec4(abs(tex))");
    }
}

#[test]
fn builtin_names_obey_versioned_scope_and_shadowed_names_are_not_callable() {
    near(render(100, "vec4(1.0)"), [255; 4], "control");
    let mut c = context(100);
    let (vs, fs) = sources(100, "uniform float sin;", "vec4(sin)");
    scene(&mut c, &vs, &fs);
    assert_eq!(pixel(&mut c), [0; 4]);
    for version in [100, 300] {
        rejects(version, "", "sin(0.0,0.0,0.0)");
        let mut c = context(version);
        let (vs, fs) = sources(version, "", "vec4(sin)");
        let fs = fs.replace("void main(){", "void main(){float sin=0.25;");
        scene(&mut c, &vs, &fs);
        near(pixel(&mut c), [64; 4], "local variable hides builtin");
        // A local variable may hide an outer built-in, but cannot be called.
        let mut c = context(version);
        let (_, mut fs) = sources(version, "", "vec4(sin(0.0))");
        fs = fs.replace("void main(){", "void main(){float sin=0.0;");
        let s = c.create_shader(FRAGMENT_SHADER).unwrap();
        c.shader_source(s, &fs).unwrap();
        c.compile_shader(s).unwrap();
        assert!(!c.shader_compile_status(s).unwrap());
        assert!(c.shader_info_log(s).unwrap().contains("not a callable"));
    }
    assert!(rejects(300, "uniform float sin;", "vec4(sin)").contains("built-in"));
    rejects(
        100,
        "uniform sampler2D tex;",
        "mix(texture(tex,vec2(0.0)),vec4(1.0),0.5)",
    );
    rejects(
        300,
        "uniform sampler2D tex;",
        "mix(texture2D(tex,vec2(0.0)),vec4(1.0),0.5)",
    );
}

#[test]
fn undefined_math_domains_do_not_panic_and_math_work_remains_bounded() {
    for version in [100, 300] {
        for expression in [
            "clamp(vec4(0.5),1.0,0.0)",
            "smoothstep(0.0,0.0,vec4(0.0))",
            "vec4(sqrt(-1.0))",
            "vec4(normalize(vec3(0.0)),1.0)",
        ] {
            // GLSL does not define these results; assert only safe bounded execution.
            let _ = render(version, expression);
        }
        let mut c = context(version);
        let (_, fs) = sources(version, "", "vec4(x)");
        let fs = fs.replace(
            "void main(){",
            &format!("void main(){{float x=0.5;{}", "x=sin(x);".repeat(110)),
        );
        let s = c.create_shader(FRAGMENT_SHADER).unwrap();
        c.shader_source(s, &fs).unwrap();
        c.compile_shader(s).unwrap();
        assert!(!c.shader_compile_status(s).unwrap());
        assert!(c.shader_info_log(s).unwrap().contains("budget"));
    }
}
