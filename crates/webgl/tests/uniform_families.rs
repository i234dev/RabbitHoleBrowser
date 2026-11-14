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
fn float_uniform_family_width_is_checked_independently_of_buffer_length() {
    for version in [100,300] {
        let mut c=context(version,1);
        let fs=fragment(version,"uniform bool flag;uniform float gain;uniform vec2 pair;uniform vec4 tint;",&format!("if(flag){{{}=tint*gain;}}else{{{}=vec4(pair,0.0,1.0);}}",output(version),output(version)));
        let(p,_)=scene(&mut c,version,None,&fs);
        let flag=c.get_uniform_location(p,"flag").unwrap().unwrap();
        let gain=c.get_uniform_location(p,"gain").unwrap().unwrap();
        let pair=c.get_uniform_location(p,"pair").unwrap().unwrap();
        let tint=c.get_uniform_location(p,"tint").unwrap().unwrap();
        c.uniform_f32_components(flag,1,&[1.]).unwrap();
        c.uniform_f32_components(gain,1,&[0.5]).unwrap();
        c.uniform_f32_components(pair,2,&[0.25,0.5]).unwrap();
        c.uniform_f32_components(tint,4,&[1.,0.5,0.25,1.]).unwrap();
        near(draw(&mut c,0),[128,64,32,128]);
        assert_eq!(c.uniform_f32_components(tint,2,&[0.;4]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_f32_components(tint,1,&[0.;4]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_f32_components(pair,1,&[0.;2]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_f32_components(flag,2,&[0.]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_f32_components(gain,4,&[0.]),Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p,flag).unwrap(),[1.]);
        assert_eq!(c.get_uniform(p,gain).unwrap(),[0.5]);
        assert_eq!(c.get_uniform(p,pair).unwrap(),[0.25,0.5]);
        assert_eq!(c.get_uniform(p,tint).unwrap(),[1.,0.5,0.25,1.]);
        near(draw(&mut c,0),[128,64,32,128]);
    }
}

#[test]
fn malformed_uniform_family_commands_are_atomic_and_null_locations_are_ignored() {
    for version in [100,300] {
        let mut c=context(version,1);
        let(p,_)=scene(&mut c,version,None,&fragment(version,"uniform float gain;",&format!("{}=vec4(gain);",output(version))));
        let gain=c.get_uniform_location(p,"gain").unwrap().unwrap();
        c.uniform_f32_components(gain,1,&[0.5]).unwrap();
        for width in [0,5,u32::MAX] {assert_eq!(c.uniform_f32_components(gain,width,&[0.]),Err(Error::InvalidValue));}
        assert_eq!(c.uniform_f32_components(gain,1,&[]),Err(Error::InvalidOperation));
        assert_eq!(c.uniform_f32_components(gain,1,&[0.,0.]),Err(Error::InvalidOperation));
        c.uniform_f32_components(-1,0,&[]).unwrap();
        assert_eq!(c.get_uniform(p,gain).unwrap(),[0.5]);
        near(draw(&mut c,0),[128;4]);
    }
}
