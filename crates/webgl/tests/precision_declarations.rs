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
