use rabbithole_webgl::{gl::*, Context};

fn context(version: u32) -> Context {
    if version == 300 { Context::new_webgl2(1, 1).unwrap() } else { Context::new(1, 1).unwrap() }
}
fn prefix(version: u32) -> &'static str { if version == 300 { "#version 300 es\n" } else { "" } }
fn shader(c: &mut Context, kind: u32, source: &str) -> u32 {
    let shader = c.create_shader(kind).unwrap();
    c.shader_source(shader, source).unwrap(); c.compile_shader(shader).unwrap();
    assert!(c.shader_compile_status(shader).unwrap(), "{source}: {}", c.shader_info_log(shader).unwrap());
    shader
}
fn vertex(version: u32) -> String {
    format!("{}\n#define PROJECT(v) vec4((v),0.0,1.0)\n#if __VERSION__>=300\nin vec2 p;\n#else\nattribute vec2 p;\n#endif\nvoid main(){{gl_Position=PROJECT(p);}}", prefix(version))
}
fn fragment(version: u32, directives: &str, expression: &str) -> String {
    format!("{}{directives}\nprecision mediump float;\n#if __VERSION__>=300\nout vec4 color;\n#define OUTPUT color\n#else\n#define OUTPUT gl_FragColor\n#endif\nvoid main(){{OUTPUT={expression};}}", prefix(version))
}
fn scene(c: &mut Context, vertex: &str, fragment: &str) -> (u32, u32) {
    let v = shader(c, VERTEX_SHADER, vertex); let f = shader(c, FRAGMENT_SHADER, fragment);
    let p = c.create_program(); c.attach_shader(p, v).unwrap(); c.attach_shader(p, f).unwrap(); c.link_program(p).unwrap();
    assert!(c.program_link_status(p).unwrap(), "{}", c.program_info_log(p).unwrap()); c.use_program(p).unwrap();
    let b = c.create_buffer(); c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    let bytes: Vec<_> = [-1f32, -1., 3., -1., -1., 3.].into_iter().flat_map(f32::to_le_bytes).collect();
    c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
    let location = c.get_attrib_location(p, "p").unwrap() as u32;
    c.vertex_attrib_pointer(location, 2, FLOAT, false, 0, 0).unwrap(); c.enable_vertex_attrib_array(location, true).unwrap();
    (p, f)
}
fn pixel(c: &mut Context) -> [u8; 4] {
    c.draw_arrays(TRIANGLES, 0, 3).unwrap(); c.read_pixels(0, 0, 1, 1).unwrap().try_into().unwrap()
}

#[test]
fn macro_arguments_rescan_in_vertex_and_fragment_shaders_to_real_pixels() {
    for version in [100, 300] {
        let mut c = context(version);
        let fs = fragment(version, "#define GAIN 0.25\n#define COPY GAIN\n#define TWICE(x) ((x)+(x))\n#define SHADE(r,g) vec4((r),(g),0.0,1.0)\n#define ALIAS SHADE\n#pragma optimize(off)\n", "ALIAS(TWICE(COPY),GAIN)");
        scene(&mut c, &vertex(version), &fs);
        assert_eq!(pixel(&mut c), [128, 64, 0, 255]);
    }
}

#[test]
fn conditional_code_and_samplers_disappear_only_when_the_branch_is_excluded() {
    for version in [100, 300] {
        let mut c = context(version);
        let fs = fragment(version, "#define COUNT 2\n#if 0 && MISSING\nuniform sampler2D unused;\n☃ invalid code\n#endif\n#if defined(COUNT) && COUNT==2 && GL_ES && GL_FRAGMENT_PRECISION_HIGH\n#define SHADE vec4(0.0,1.0,0.0,1.0)\n#elif UNDEFINED\n#error wrong branch evaluated\n#else\n#error no branch\n#endif\n", "SHADE");
        let (p, _) = scene(&mut c, &vertex(version), &fs);
        assert!(c.active_uniforms(p).unwrap().is_empty());
        assert_eq!(pixel(&mut c), [0, 255, 0, 255]);
    }
}

#[test]
fn line_and_version_builtins_are_evaluated_at_the_invocation_site() {
    for version in [100, 300] {
        let line = if version == 300 { 3 } else { 2 };
        let directives = format!("#define HERE __LINE__\n#if HERE=={line} && __VERSION__=={version} && __FILE__==0\n#define SHADE vec4(0.0,1.0,1.0,1.0)\n#else\n#error wrong builtins\n#endif\n");
        let mut c = context(version); scene(&mut c, &vertex(version), &fragment(version, &directives, "SHADE"));
        assert_eq!(pixel(&mut c), [0, 255, 255, 255]);
    }
}

#[test]
fn three_style_version_aliases_preserve_real_texture_sampling() {
    for version in [100, 300] {
        let vs = format!("{}\n#if __VERSION__>=300\n#define attribute in\n#define varying out\n#endif\nattribute vec2 p;varying vec2 uv;void main(){{gl_Position=vec4(p,0.0,1.0);uv=p*0.5+0.5;}}", prefix(version));
        let fs = format!("{}\nprecision mediump float;\n#if __VERSION__>=300\n#define varying in\n#define texture2D texture\nout vec4 outputColor;\n#define gl_FragColor outputColor\n#endif\nvarying vec2 uv;uniform sampler2D image;\n#define FETCH(t,u) texture2D(t,u)\nvoid main(){{gl_FragColor=FETCH(image,uv);}}", prefix(version));
        let mut c = context(version); scene(&mut c, &vs, &fs);
        let texture = c.create_texture(); c.bind_texture(TEXTURE_2D, texture).unwrap();
        c.tex_image_2d(TEXTURE_2D, 0, RGBA, 1, 1, 0, RGBA, UNSIGNED_BYTE, Some(&[20, 200, 60, 255])).unwrap();
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST as i32).unwrap();
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_MAG_FILTER, NEAREST as i32).unwrap();
        assert_eq!(pixel(&mut c), [20, 200, 60, 255]);
    }
}

#[test]
fn continued_comments_and_discarded_recursive_macro_arguments_do_not_leak_tokens() {
    for version in [100, 300] {
        // Three directives exercise the MIT-licensed Khronos tricky expansion case.
        // The bounded shader program around them is our own pixel regression.
        let directives = "// removed\\\n#error line continuation did not happen\n#define m(a)\n#define a m((a)\na)\n#define FLOAT 0.5\n";
        let mut c = context(version); scene(&mut c, &vertex(version), &fragment(version, directives, "vec4(FLOAT,0.0,FLOAT,1.0)"));
        assert_eq!(pixel(&mut c), [128, 0, 128, 255]);
    }
}

#[test]
fn preprocessing_failures_are_compile_errors_and_keep_the_linked_executable() {
    for version in [100, 300] {
        let mut c = context(version);
        let (_, f) = scene(&mut c, &vertex(version), &fragment(version, "", "vec4(0.0,1.0,0.0,1.0)"));
        for bad in ["#define F(x,x) x\n", "#define F(x) x\n#define V F(1.0,2.0)\nV\n", "#if MISSING\n#endif\n", "#define D defined\n#if D(X)\n#endif\n", "#extension all : disable\n", "#line 20\n", "#error expected rejection\n"] {
            c.shader_source(f, &fragment(version, bad, "vec4(1.0,0.0,0.0,1.0)")).unwrap();
            c.compile_shader(f).unwrap();
            assert!(!c.shader_compile_status(f).unwrap(), "{bad}");
            assert!(!c.shader_info_log(f).unwrap().is_empty(), "{bad}");
            assert_eq!(pixel(&mut c), [0, 255, 0, 255], "{bad}");
        }
    }
}

#[test]
fn a_block_comment_newline_starts_a_directive_in_essl100_but_not_300() {
    let source100 = "precision mediump float; /* comment\n*/ #define COLOR vec4(1.0)\nvoid main(){gl_FragColor=COLOR;}";
    let mut old = context(100); scene(&mut old, &vertex(100), source100);
    assert_eq!(pixel(&mut old), [255, 255, 255, 255]);

    let source300 = "#version 300 es\nprecision mediump float;out vec4 color; /* comment\n*/ #define COLOR vec4(1.0)\nvoid main(){color=COLOR;}";
    let mut new = context(300); let fragment = new.create_shader(FRAGMENT_SHADER).unwrap();
    new.shader_source(fragment, source300).unwrap(); new.compile_shader(fragment).unwrap();
    assert!(!new.shader_compile_status(fragment).unwrap());
    assert!(new.shader_info_log(fragment).unwrap().contains("# must start a directive"));
}
