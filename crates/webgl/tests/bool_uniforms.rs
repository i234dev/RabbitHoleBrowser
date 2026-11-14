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


fn bool_scene(c:&mut Context,version:u32)->(u32,u32) {
    let fs=fragment(version,"uniform bool enabled;bool readEnabled(){return enabled;}vec4 shade(bool enabled){if(enabled)return vec4(1.0,0.0,0.0,1.0);return vec4(0.0,1.0,0.0,1.0);}",&format!("{}=shade(readEnabled());",output(version)));
    scene(c,version,None,&fs)
}

#[test]
fn boolean_uniforms_default_false_and_normalize_integer_and_float_updates() {
    for version in [100,300] {
        let mut c=context(version,1);
        let (p,_)=bool_scene(&mut c,version);
        let loc=c.get_uniform_location(p,"enabled").unwrap().unwrap();
        let info=c.active_uniforms(p).unwrap();
        assert_eq!(info.len(),1);assert_eq!(info[0].name,"enabled");
        assert_eq!(info[0].kind,BOOL);assert_eq!(info[0].size,1);
        assert_eq!(c.get_uniform_kind(p,loc).unwrap(),BOOL);
        assert_eq!(c.get_uniform(p,loc).unwrap(),[0.]);
        assert_eq!(draw(&mut c,0),[0,255,0,255]);
        for value in [1,0,-1,i32::MIN,i32::MAX,0,100] {
            c.uniform_i32(loc,&[value]).unwrap();
            let truth=value!=0;
            assert_eq!(c.get_uniform(p,loc).unwrap(),[u8::from(truth) as f32]);
            assert_eq!(draw(&mut c,0),if truth {[255,0,0,255]}else{[0,255,0,255]});
        }
        for value in [0.0,-0.0,0.25,-1.0,f32::NAN,f32::INFINITY,f32::NEG_INFINITY,0.0] {
            c.uniform_f32(loc,&[value]).unwrap();
            let truth=value!=0.;
            assert_eq!(c.get_uniform(p,loc).unwrap(),[u8::from(truth) as f32]);
            assert_eq!(draw(&mut c,0),if truth {[255,0,0,255]}else{[0,255,0,255]});
        }
    }
}

#[test]
fn invalid_bool_setters_preserve_values_and_pixels() {
    for version in [100,300] {
        let mut c=context(version,1);let(p,_)=bool_scene(&mut c,version);
        let loc=c.get_uniform_location(p,"enabled").unwrap().unwrap();
        c.uniform_i32(loc,&[-50]).unwrap();
        assert_eq!(draw(&mut c,0),[255,0,0,255]);
        assert_eq!(c.uniform_i32(loc,&[0,0]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_i32(loc,&[]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_f32(loc,&[0.,0.]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_f32(loc,&[]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_matrix4fv(loc,false,&[0.;16]),Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p,loc).unwrap(),[1.]);
        assert_eq!(draw(&mut c,0),[255,0,0,255]);
        c.uniform_i32(-1,&[0,0]).unwrap();c.uniform_f32(-1,&[0.,0.]).unwrap();
        assert_eq!(c.get_uniform(p,loc).unwrap(),[1.]);
    }
}

#[test]
fn bool_and_sampler_integer_setters_keep_distinct_types_and_ranges() {
    for version in [100,300] {
        let mut c=context(version,1);
        let sample=if version==300{"texture"}else{"texture2D"};
        let fs=fragment(version,"uniform bool enabled;uniform sampler2D image;uniform float gain;",&format!("if(enabled){{{}={sample}(image,vec2(0.5));}}else{{{}=vec4(gain);}}",output(version),output(version)));
        let(p,_)=scene(&mut c,version,None,&fs);
        let flag=c.get_uniform_location(p,"enabled").unwrap().unwrap();
        let image=c.get_uniform_location(p,"image").unwrap().unwrap();
        let gain=c.get_uniform_location(p,"gain").unwrap().unwrap();
        c.uniform_i32(flag,&[i32::MAX]).unwrap();
        c.uniform_i32(image,&[0]).unwrap();
        assert_eq!(c.uniform_i32(image,&[8]),Err(Error::InvalidValue));
        assert_eq!(c.uniform_i32(image,&[-1]),Err(Error::InvalidValue));
        assert_eq!(c.uniform_f32(image,&[1.]),Err(Error::InvalidOperation));
        c.uniform_f32(gain,&[0.5]).unwrap();
        assert_eq!(c.uniform_i32(gain,&[1]),Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p,flag).unwrap(),[1.]);
        assert_eq!(c.get_uniform(p,image).unwrap(),[0.]);
        assert_eq!(c.get_uniform(p,gain).unwrap(),[0.5]);
        c.uniform_i32(flag,&[0]).unwrap();near(draw(&mut c,0),[128;4]);
    }
}

#[test]
fn relink_resets_bool_defaults_and_invalidates_old_locations() {
    for version in [100,300] {
        let mut c=context(version,1);let(p,_)=bool_scene(&mut c,version);
        let old=c.get_uniform_location(p,"enabled").unwrap().unwrap();
        c.uniform_i32(old,&[1]).unwrap();assert_eq!(draw(&mut c,0),[255,0,0,255]);
        let(other,_)=bool_scene(&mut c,version);
        assert_eq!(c.uniform_i32(old,&[0]),Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p,old).unwrap(),[1.]);
        assert_eq!(c.get_uniform_kind(other,old),Err(Error::InvalidOperation));
        c.link_program(p).unwrap();assert!(c.program_link_status(p).unwrap());
        c.use_program(p).unwrap();
        let fresh=c.get_uniform_location(p,"enabled").unwrap().unwrap();
        assert_ne!(fresh,old);
        assert_eq!(c.uniform_i32(old,&[1]),Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p,old),Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p,fresh).unwrap(),[0.]);
        assert_eq!(draw(&mut c,0),[0,255,0,255]);
    }
}

#[test]
fn boolean_uniforms_link_across_stages_and_mismatched_types_fail() {
    for version in [100,300] {
        let mut c=context(version,1);
        let vs=format!("{}{} vec2 position;uniform bool enabled;void main(){{if(enabled){{gl_Position=vec4(position,0.0,1.0);}}else{{gl_Position=vec4(position,0.0,1.0);}}}}",if version==300{"#version 300 es\n"}else{""},if version==300{"in"}else{"attribute"});
        let fs=fragment(version,"uniform bool enabled;",&format!("if(enabled){{{}=vec4(1.0,0.0,0.0,1.0);}}else{{{}=vec4(0.0,1.0,0.0,1.0);}}",output(version),output(version)));
        let(p,s)=scene(&mut c,version,Some(&vs),&fs);
        assert_eq!(c.active_uniforms(p).unwrap().len(),1);
        let enabled=c.get_uniform_location(p,"enabled").unwrap().unwrap();
        c.uniform_i32(enabled,&[1]).unwrap();assert_eq!(draw(&mut c,0),[255,0,0,255]);
        let mismatched=fragment(version,"uniform float enabled;",&format!("{}=vec4(enabled);",output(version)));
        c.shader_source(s,&mismatched).unwrap();c.compile_shader(s).unwrap();
        assert!(c.shader_compile_status(s).unwrap());c.link_program(p).unwrap();
        assert!(!c.program_link_status(p).unwrap());
        assert!(c.program_info_log(p).unwrap().contains("uniform type mismatch"));
        assert_eq!(c.draw_arrays(TRIANGLES,0,3),Err(Error::InvalidOperation));
        assert_eq!(c.read_pixels(0,0,1,1).unwrap(),[255,0,0,255]);
    }
}

#[test]
fn unused_boolean_helpers_do_not_activate_uniforms_and_uniforms_remain_read_only() {
    for version in [100,300] {
        let mut c=context(version,1);
        let fs=fragment(version,"uniform bool hidden;bool uncalled(){return hidden;}",&format!("bool hidden=false;{}=vec4(1.0);",output(version)));
        let(p,_)=scene(&mut c,version,None,&fs);
        assert_eq!(c.get_uniform_location(p,"hidden").unwrap(),None);
        assert!(c.active_uniforms(p).unwrap().is_empty());assert_eq!(draw(&mut c,0),[255;4]);
        for extra in ["uniform bool hidden;bool f(){hidden=true;return hidden;}","uniform lowp bool hidden;"] {
            let src=fragment(version,extra,&format!("{}=vec4(1.0);",output(version)));
            let s=compile(&mut c,FRAGMENT_SHADER,&src);assert!(!c.shader_compile_status(s).unwrap());
        }
        let src=fragment(version,"uniform bool hidden;",&format!("hidden=true;{}=vec4(1.0);",output(version)));
        let s=compile(&mut c,FRAGMENT_SHADER,&src);assert!(!c.shader_compile_status(s).unwrap());
    }
}
