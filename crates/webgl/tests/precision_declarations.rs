use rabbithole_webgl::{gl::*, Context};

fn compile(context: &mut Context, kind: u32, source: &str) -> u32 {
    let shader = context.create_shader(kind).unwrap();
    context.shader_source(shader, source).unwrap();
    context.compile_shader(shader).unwrap();
    shader
}

fn fragment(version: u32, declarations: &str) -> String {
    if version == 300 {
        format!("#version 300 es\nprecision highp float;{declarations}out vec4 color;void main(){{color=vec4(0.0,1.0,0.0,1.0);}}")
    } else {
        format!("precision highp float;{declarations}void main(){{gl_FragColor=vec4(0.0,1.0,0.0,1.0);}}")
    }
}

#[test]
fn versioned_default_sampler_precisions_do_not_instantiate_unsupported_uniforms() {
    let types = ["sampler2D", "samplerCube", "sampler3D", "sampler2DArray", "sampler2DShadow", "samplerCubeShadow", "sampler2DArrayShadow", "isampler2D", "isampler3D", "isamplerCube", "isampler2DArray", "usampler2D", "usampler3D", "usamplerCube", "usampler2DArray"];
    for version in [100, 300] {
        let mut context = if version == 300 { Context::new_webgl2(1, 1).unwrap() } else { Context::new(1, 1).unwrap() };
        for (index, ty) in types.iter().enumerate() {
            let shader = compile(&mut context, FRAGMENT_SHADER, &fragment(version, &format!("precision highp {ty};")));
            assert_eq!(context.shader_compile_status(shader).unwrap(), version == 300 || index < 2, "{version} {ty}: {}", context.shader_info_log(shader).unwrap());
        }
        for ty in ["bool", "uint", "vec4", "mat4", "sampler2DMS", "samplerBuffer", "samplerExternalOES"] {
            let shader = compile(&mut context, FRAGMENT_SHADER, &fragment(version, &format!("precision highp {ty};")));
            assert!(!context.shader_compile_status(shader).unwrap(), "invalid precision type {ty}");
        }
        let shader = compile(&mut context, FRAGMENT_SHADER, &fragment(version, "precision highp samplerCube;uniform samplerCube cube;"));
        assert!(!context.shader_compile_status(shader).unwrap(), "unsupported cube uniforms must still fail");
    }
}

#[test]
fn validation_is_historical_per_program_and_rejects_excess_active_samplers() {
    let mut context = Context::new_webgl2(1, 1).unwrap();
    let program = context.create_program();
    assert!(!context.program_validation_status(program).unwrap());
    context.validate_program(program).unwrap();
    assert!(!context.program_validation_status(program).unwrap());
    assert!(!context.program_info_log(program).unwrap().is_empty());
    let vertex = compile(&mut context, VERTEX_SHADER, "#version 300 es\nvoid main(){gl_Position=vec4(0.0,0.0,0.0,1.0);}");
    let fragment_shader = compile(&mut context, FRAGMENT_SHADER, &fragment(300, ""));
    for shader in [vertex, fragment_shader] { context.attach_shader(program, shader).unwrap(); }
    context.link_program(program).unwrap();
    assert!(context.program_link_status(program).unwrap(), "{}", context.program_info_log(program).unwrap());
    assert!(!context.program_validation_status(program).unwrap());
    context.validate_program(program).unwrap();
    assert!(context.program_validation_status(program).unwrap());
    assert_eq!(context.program_info_log(program).unwrap(), "");

    // Relinking does not implicitly validate. The last explicit result stays
    // true until a new attempt inspects the now-unlinked program.
    context.shader_source(fragment_shader, "not a shader").unwrap();
    context.compile_shader(fragment_shader).unwrap();
    context.link_program(program).unwrap();
    assert!(!context.program_link_status(program).unwrap());
    assert!(context.program_validation_status(program).unwrap());
    context.validate_program(program).unwrap();
    assert!(!context.program_validation_status(program).unwrap());

    let count = rabbithole_webgl::MAX_TEXTURE_UNITS + 1;
    let mut source = String::from("#version 300 es\nprecision highp float;out vec4 color;");
    for i in 0..count { source.push_str(&format!("uniform sampler2D s{i};")); }
    source.push_str("void main(){color=vec4(0.0);");
    for i in 0..count { source.push_str(&format!("color=color+texture(s{i},vec2(0.0));")); }
    source.push('}');
    let excess_fragment = compile(&mut context, FRAGMENT_SHADER, &source);
    assert!(context.shader_compile_status(excess_fragment).unwrap(), "{}", context.shader_info_log(excess_fragment).unwrap());
    let excess = context.create_program();
    for shader in [vertex, excess_fragment] { context.attach_shader(excess, shader).unwrap(); }
    context.link_program(excess).unwrap();
    assert!(context.program_link_status(excess).unwrap(), "{}", context.program_info_log(excess).unwrap());
    context.validate_program(excess).unwrap();
    assert!(!context.program_validation_status(excess).unwrap());
    assert!(context.program_info_log(excess).unwrap().contains("sampler count"));
    assert_eq!(context.validate_program(u32::MAX), Err(rabbithole_webgl::Error::InvalidValue));
}

#[test]
fn uniform_precision_is_captured_at_declaration_and_matched_across_both_versions() {
    for version in [100, 300] {
    for (fragment_precision, expected) in [("highp", true), ("mediump", false)] {
        let mut context = if version == 300 { Context::new_webgl2(1, 1).unwrap() } else { Context::new(1, 1).unwrap() };
        let prefix = if version == 300 { "#version 300 es\n" } else { "" };
        let vertex = compile(&mut context, VERTEX_SHADER, &format!("{prefix}precision highp float;uniform float gain;precision mediump float;void main(){{gl_Position=vec4(gain,0.0,0.0,1.0);}}"));
        let (declaration, output) = if version == 300 { ("out vec4 color;", "color") } else { ("", "gl_FragColor") };
        let source = format!("{prefix}precision {fragment_precision} float;uniform float gain;precision lowp float;{declaration}void main(){{{output}=vec4(gain);}}");
        let fragment = compile(&mut context, FRAGMENT_SHADER, &source);
        for shader in [vertex, fragment] { assert!(context.shader_compile_status(shader).unwrap(), "{}", context.shader_info_log(shader).unwrap()); }
        let program = context.create_program();
        for shader in [vertex, fragment] { context.attach_shader(program, shader).unwrap(); }
        context.link_program(program).unwrap();
        assert_eq!(context.program_link_status(program).unwrap(), expected, "{}", context.program_info_log(program).unwrap());
        if !expected { assert!(context.program_info_log(program).unwrap().contains("uniform precision mismatch")); }
    }
    }
}
