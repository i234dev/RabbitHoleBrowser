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


fn reject(version: u32, extra: &str, body: &str) -> String {
    let mut c=context(version,1);
    let source=fragment(version,extra,body);
    let shader=compile(&mut c,FRAGMENT_SHADER,&source);
    assert!(!c.shader_compile_status(shader).unwrap(),"accepted invalid shader: {source}");
    let log=c.shader_info_log(shader).unwrap().to_string();
    assert!(!log.is_empty());
    log
}

#[test]
fn scalar_boolean_operators_precedence_and_aggregate_equality_produce_pixels() {
    for version in [100,300] {
        for expression in [
            "true", "!false", "bool(true)", "true==!false", "false!=true",
            "false||true&&true", "true||false&&false", "false^^true&&true",
            "true^^false||false", "!(true^^true)", "!(false&&true)",
            "0.25+0.25*2.0<1.0", "0.5<=0.5", "0.75>0.5", "0.5>=0.5",
            "-1.0*2.0<-1.0", "1.0+1.0==2.0", "vec3(1.0)==vec3(1.0)",
            "vec2(1.0,0.0)!=vec2(0.0,1.0)", "mat4(1.0)==mat4(1.0)",
            "mat4(1.0)!=mat4(0.0)", "!(vec4(1.0)==vec4(0.0))",
        ] {
            let mut c=context(version,1);
            let out=output(version);
            let fs=fragment(version,"",&format!("bool ok={expression};if(ok){{{out}=vec4(1.0,0.0,0.0,1.0);}}else{{{out}=vec4(0.0,1.0,0.0,1.0);}}"));
            scene(&mut c,version,None,&fs);
            assert_eq!(draw(&mut c,0),[255,0,0,255],"{expression}");
        }
    }
}

#[test]
fn per_fragment_selection_and_early_main_returns_draw_distinct_pixels() {
    for version in [100,300] {
        let mut c=context(version,2);
        let out=output(version);
        let fs=fragment(version,"",&format!("if(gl_FragCoord.x<1.0){{{out}=vec4(1.0,0.0,0.0,1.0);return;}}{out}=vec4(0.0,1.0,0.0,1.0);"));
        let vs=format!("{}{} vec2 position;vec4 project(vec2 p){{if(p.x<0.0)return vec4(p,0.0,1.0);return vec4(p,0.0,1.0);}}void main(){{gl_Position=project(position);if(position.x<0.0)return;gl_Position=vec4(position,0.0,1.0);}}",if version==300{"#version 300 es\n"}else{""},if version==300{"in"}else{"attribute"});
        scene(&mut c,version,Some(&vs),&fs);
        assert_eq!(draw(&mut c,0),[255,0,0,255]);
        assert_eq!(draw(&mut c,1),[0,255,0,255]);
    }
}

#[test]
fn typed_boolean_helpers_and_both_returning_branches_execute() {
    for version in [100,300] {
        let mut c=context(version,2);
        let fs=fragment(version,"bool left(float x){if(x>=0.0&&x<1.0)return true;return false;}vec4 shade(in bool selected){if(selected){return vec4(1.0,0.0,0.0,1.0);}else{return vec4(0.0,1.0,0.0,1.0);}}",&format!("{}=shade(left(gl_FragCoord.x));",output(version)));
        scene(&mut c,version,None,&fs);
        assert_eq!(draw(&mut c,0),[255,0,0,255]);
        assert_eq!(draw(&mut c,1),[0,255,0,255]);
    }
}

#[test]
fn nested_block_bindings_shadow_globals_parameters_and_siblings_without_leaking() {
    for version in [100,300] {
        let mut c=context(version,1);
        let fs=fragment(version,"uniform float gain;float readGain(){return gain;}float value(float gain){float x=gain;{float gain=gain+readGain();x=x+gain;}{float gain=0.125;x=x+gain;}return x+gain;}",&format!("float gain=gain;float blue=0.0;if(true){{float gain=0.5;blue=gain;}}{{vec2 gain=vec2(0.0);}}{}=vec4(value(0.125),gain,blue,1.0);",output(version)));
        let (p,_)=scene(&mut c,version,None,&fs);
        let gain=c.get_uniform_location(p,"gain").unwrap().unwrap();
        c.uniform_f32(gain,&[0.25]).unwrap();
        near(draw(&mut c,0),[191,64,128,255]);
    }
}

#[test]
fn dangling_else_binds_to_nearest_if_and_optional_else_keeps_prior_output() {
    for version in [100,300] {
        let mut c=context(version,1);
        let out=output(version);
        let fs=fragment(version,"",&format!("{out}=vec4(0.0,0.0,1.0,1.0);if(true)if(false){out}=vec4(1.0,0.0,0.0,1.0);else {out}=vec4(0.0,1.0,0.0,1.0);if(false){{{out}=vec4(1.0);}}"));
        scene(&mut c,version,None,&fs);
        assert_eq!(draw(&mut c,0),[0,255,0,255]);
    }
}

#[test]
fn bool_types_do_not_become_float_math_numeric_conditions_or_interfaces() {
    for version in [100,300] {
        let out=output(version);
        for body in [
            "if(1.0){}", "if(vec2(1.0)){}", "bool b=1.0;", "float f=true;",
            "bool b=true+false;", "bool b=-true;", "bool b=+false;",
            "bool b=!1.0;", "bool b=true&&1.0;", "bool b=1.0||true;",
            "bool b=vec2(1.0)<vec2(2.0);", "bool b=1<2;", "bool b=true==1.0;",
            "bool b=vec2(1.0)==vec3(1.0);", "float f=sin(true);", "float f=float(true);",
            "bool b=bool(1.0);", "lowp bool b=true;", "vec3 v=vec3(true);", "float f=true.x;",
            "bool b=true||unknown;", "bool b=false&&unknown;",
        ] {
            reject(version,"",&format!("{body}{out}=vec4(1.0);"));
        }
        for extra in ["uniform bool flag;",if version==300{"in bool flag;"}else{"varying bool flag;"}] {
            assert!(reject(version,extra,&format!("{out}=vec4(1.0);")).contains("bool interfaces"));
        }
        let mut c=context(version,1);
        let vs=if version==300{"#version 300 es\nin bool flag;void main(){gl_Position=vec4(0.0);}"}else{"attribute bool flag;void main(){gl_Position=vec4(0.0);}"};
        let s=compile(&mut c,VERTEX_SHADER,vs);
        assert!(!c.shader_compile_status(s).unwrap());
        assert!(c.shader_info_log(s).unwrap().contains("bool interfaces"));
        reject(version,"bool f(bool x){return 0.0;}",&format!("{out}=vec4(1.0);"));
        reject(version,"bool f(bool x){return x;}",&format!("bool b=f(1.0);{out}=vec4(1.0);"));
    }
}

#[test]
fn block_names_are_not_visible_after_scope_exit_or_during_their_initializer() {
    for version in [100,300] {
        let out=output(version);
        for body in [
            "{float hidden=0.5;}float f=hidden;",
            "if(true)float hidden=0.5;float f=hidden;",
            "if(true){float hidden=0.5;}else{float f=hidden;}",
            "{float x=x;}", "{float x=0.0;float x=0.5;}",
        ] { reject(version,"",&format!("{body}{out}=vec4(1.0);")); }
        reject(version,"float f(float x){float x=0.0;return x;}",&format!("{out}=vec4(1.0);"));
        reject(version,"uniform float gain;float f(bool b){if(b){gain=0.5;}return gain;}",&format!("{out}=vec4(1.0);"));
        reject(version,&format!("vec4 f(bool b){{if(b)return {out};return vec4(1.0);}}"),&format!("{out}=vec4(1.0);"));
    }
}

#[test]
fn every_main_exit_writes_output_and_every_helper_path_returns() {
    for version in [100,300] {
        let out=output(version);
        for body in [
            format!("if(true){{{out}=vec4(1.0);}}"),
            format!("if(true)return;{out}=vec4(1.0);"),
            format!("if(true){{{out}=vec4(1.0);return;}}else{{return;}}"),
            format!("return;{out}=vec4(1.0);"),
            format!("return vec4(1.0);{out}=vec4(1.0);"),
        ] { reject(version,"",&body); }
        for helper in [
            "float f(bool b){if(b)return 0.5;}",
            "float f(bool b){if(b)return 0.5;else return true;}",
            "float f(bool b){if(b)return;return 0.5;}",
            "float f(bool b){if(b)return 0.5;else return 1.0;return 0.0;}",
            "float f(bool b){if(b){return 0.5;}else{float x=1.0;}}",
        ] { reject(version,helper,&format!("{out}=vec4(1.0);")); }
    }
}

fn texture(c:&mut Context,unit:u32,color:[u8;4])->u32 {
    c.active_texture(TEXTURE0+unit).unwrap();
    let t=c.create_texture();c.bind_texture(TEXTURE_2D,t).unwrap();
    for filter in [TEXTURE_MIN_FILTER,TEXTURE_MAG_FILTER] {c.tex_parameter_i(TEXTURE_2D,filter,NEAREST as i32).unwrap();}
    c.tex_image_2d(TEXTURE_2D,0,RGBA,1,1,0,RGBA,UNSIGNED_BYTE,Some(&color)).unwrap();
    t
}

#[test]
fn sampler_reflection_covers_both_return_branches_and_feedback_is_checked_atomically() {
    for version in [100,300] {
        let mut c=context(version,1);
        let sample=if version==300{"texture"}else{"texture2D"};
        let fs=fragment(version,&format!("uniform sampler2D first;uniform sampler2D second;uniform float choose;vec4 selectColor(){{if(choose<0.5)return {sample}(first,vec2(0.5));else return {sample}(second,vec2(0.5));}}vec4 caller(float second){{return selectColor();}}"),&format!("{}=caller(0.0);",output(version)));
        let (p,_)=scene(&mut c,version,None,&fs);
        let first=c.get_uniform_location(p,"first").unwrap().unwrap();
        let second=c.get_uniform_location(p,"second").unwrap().unwrap();
        let choose=c.get_uniform_location(p,"choose").unwrap().unwrap();
        c.uniform_i32(first,&[0]).unwrap();c.uniform_i32(second,&[1]).unwrap();
        texture(&mut c,0,[255,0,0,255]);
        let green=texture(&mut c,1,[0,255,0,255]);
        c.uniform_f32(choose,&[0.0]).unwrap();assert_eq!(draw(&mut c,0),[255,0,0,255]);
        c.uniform_f32(choose,&[1.0]).unwrap();assert_eq!(draw(&mut c,0),[0,255,0,255]);
        c.clear_color([0.,0.,1.,1.]);c.clear(COLOR_BUFFER_BIT).unwrap();
        let f=c.create_framebuffer();c.bind_framebuffer(FRAMEBUFFER,f).unwrap();
        c.framebuffer_texture_2d(FRAMEBUFFER,COLOR_ATTACHMENT0,TEXTURE_2D,green,0).unwrap();
        c.uniform_f32(choose,&[0.0]).unwrap();
        assert_eq!(c.draw_arrays(TRIANGLES,0,3),Err(Error::InvalidOperation));
        assert_eq!(c.read_pixels(0,0,1,1).unwrap(),[0,255,0,255]);
        c.bind_framebuffer(FRAMEBUFFER,0).unwrap();
        assert_eq!(c.read_pixels(0,0,1,1).unwrap(),[0,0,255,255]);
    }
}

#[test]
fn constant_dead_branches_are_typed_and_reflected() {
    for version in [100,300] {
        let out=output(version);
        let mut c=context(version,1);
        let fs=fragment(version,"uniform float unusedAtRuntime;",&format!("if(false){{{out}=vec4(unusedAtRuntime);}}else{{{out}=vec4(1.0);}}"));
        let (p,_)=scene(&mut c,version,None,&fs);
        assert!(c.get_uniform_location(p,"unusedAtRuntime").unwrap().is_some());
        assert_eq!(draw(&mut c,0),[255;4]);
        reject(version,"",&format!("if(false){{float x=missing;}}{out}=vec4(1.0);"));
    }
}

#[test]
fn statement_nesting_dead_branch_work_and_call_graph_costs_remain_bounded() {
    for version in [100,300] {
        let out=output(version);
        let deep=format!("{}{}=vec4(1.0);{}","{".repeat(17),out,"}".repeat(17));
        assert!(reject(version,"",&deep).contains("nesting"));
        let body=format!("float x=0.0;if(false){{{}}}{out}=vec4(x);","x=f(x);".repeat(80));
        assert!(reject(version,"float f(float x){if(x<0.0)return 0.0;return x;}",&body).contains("budget"));
        for body in [format!("for(;;){{}}{out}=vec4(1.0);"),format!("while(true){{}}{out}=vec4(1.0);"),format!("{out}=true?vec4(1.0):vec4(0.0);")] {reject(version,"",&body);}
    }
}

#[test]
fn failed_conditional_compile_cannot_execute_a_partially_accepted_main() {
    for version in [100,300] {
        let mut c=context(version,1);
        let out=output(version);
        let valid=fragment(version,"",&format!("if(true){{{out}=vec4(1.0,0.0,0.0,1.0);}}else{{{out}=vec4(0.0);}}"));
        let (p,s)=scene(&mut c,version,None,&valid);
        assert_eq!(draw(&mut c,0),[255,0,0,255]);
        let invalid=fragment(version,"",&format!("{out}=vec4(0.0,1.0,0.0,1.0);if(1.0){{}}"));
        c.shader_source(s,&invalid).unwrap();c.compile_shader(s).unwrap();
        assert!(!c.shader_compile_status(s).unwrap());c.link_program(p).unwrap();
        assert!(!c.program_link_status(p).unwrap());
        assert_eq!(c.draw_arrays(TRIANGLES,0,3),Err(Error::InvalidOperation));
        assert_eq!(c.read_pixels(0,0,1,1).unwrap(),[255,0,0,255]);
    }
}
