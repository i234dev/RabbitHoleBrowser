use rabbithole_webgl::{gl::*, Context, Error};

fn prefix(version: u32) -> &'static str { if version == 300 { "#version 300 es\n" } else { "" } }
fn output(version: u32) -> &'static str { if version == 300 { "pixel" } else { "gl_FragColor" } }
fn fragment(version: u32, extra: &str, body: &str) -> String {
    format!("{}precision mediump float;{}{}void main(){{{}}}", prefix(version), if version == 300 { "out vec4 pixel;" } else { "" }, extra, body.replace("OUTPUT", output(version)))
}
fn shader(c: &mut Context, kind: u32, source: &str) -> u32 {
    let s = c.create_shader(kind).unwrap();c.shader_source(s, source).unwrap();c.compile_shader(s).unwrap();
    assert!(c.shader_compile_status(s).unwrap(), "{source}: {}", c.shader_info_log(s).unwrap());s
}
fn link(c: &mut Context, vertex: &str, fragment: &str) -> u32 {
    let v = shader(c, VERTEX_SHADER, vertex);let f = shader(c, FRAGMENT_SHADER, fragment);
    let p = c.create_program();c.attach_shader(p,v).unwrap();c.attach_shader(p,f).unwrap();c.link_program(p).unwrap();p
}
fn scene(version: u32, width: u32, vertex: Option<&str>, fragment: &str) -> (Context, u32) {
    let mut c = if version == 300 { Context::new_webgl2(width,1) } else { Context::new(width,1) }.unwrap();
    let default_vertex = format!("{}{} vec2 position;void main(){{gl_Position=vec4(position,0.0,1.0);}}", prefix(version), if version == 300 { "in" } else { "attribute" });
    let p = link(&mut c, vertex.unwrap_or(&default_vertex), fragment);
    assert!(c.program_link_status(p).unwrap(), "{}", c.program_info_log(p).unwrap());c.use_program(p).unwrap();
    let b = c.create_buffer();c.bind_buffer(ARRAY_BUFFER,b).unwrap();
    let bytes: Vec<_> = [-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect();c.buffer_data(ARRAY_BUFFER,&bytes).unwrap();
    let a = c.get_attrib_location(p,"position").unwrap() as u32;c.vertex_attrib_pointer(a,2,FLOAT,false,0,0).unwrap();c.enable_vertex_attrib_array(a,true).unwrap();(c,p)
}
fn draw(c: &mut Context) -> Vec<u8> { c.draw_arrays(TRIANGLES,0,3).unwrap();c.read_pixels(0,0,1,1).unwrap() }

#[test]
fn adjacent_large_integer_components_are_distinct_in_both_shader_versions() {
    for version in [100,300] {
        let source = fragment(version,"uniform highp ivec2 pair;","if(pair.x==pair.y){OUTPUT=vec4(1.0,0.0,0.0,1.0);}else{OUTPUT=vec4(0.0,1.0,0.0,1.0);}");
        let (mut c,p) = scene(version,1,None,&source);let pair = c.get_uniform_location(p,"pair").unwrap().unwrap();
        assert_eq!(c.get_uniform_i32(p,pair).unwrap(),[0,0]);assert_eq!(draw(&mut c),[255,0,0,255]);
        c.uniform_i32_components(pair,2,&[16_777_216,16_777_217]).unwrap();
        assert_eq!(c.get_uniform_i32(p,pair).unwrap(),[16_777_216,16_777_217]);assert_eq!(draw(&mut c),[0,255,0,255]);
        assert_eq!(c.get_uniform(p,pair),Err(Error::InvalidOperation));
        c.uniform_i32_components(pair,2,&[16_777_217,16_777_217]).unwrap();assert_eq!(draw(&mut c),[255,0,0,255]);
        let info = c.active_uniforms(p).unwrap();assert_eq!(info.len(),1);assert_eq!(info[0].name,"pair");assert_eq!(info[0].kind,INT_VEC2);assert_eq!(info[0].size,1);
    }
}

#[test]
fn every_integer_width_has_exact_limits_explicit_color_conversion_and_atomic_errors() {
    for version in [100,300] {
        for (width,ty,kind,body,expected) in [
            (1,"int",INT,"OUTPUT=vec4(float(value)/255.0,0.0,0.0,1.0);",[64,0,0,255]),
            (2,"ivec2",INT_VEC2,"OUTPUT=vec4(vec2(value)/255.0,0.0,1.0);",[64,128,0,255]),
            (3,"ivec3",INT_VEC3,"OUTPUT=vec4(vec3(value)/255.0,1.0);",[64,128,192,255]),
            (4,"ivec4",INT_VEC4,"OUTPUT=vec4(value)/255.0;",[64,128,192,255]),
        ] {
            let source = fragment(version,&format!("uniform highp {ty} value;"),body);
            let (mut c,p) = scene(version,1,None,&source);let value = c.get_uniform_location(p,"value").unwrap().unwrap();
            let limits = &[i32::MAX,i32::MIN,16_777_217,-16_777_217][..width];
            c.uniform_i32_components(value,width as u32,limits).unwrap();assert_eq!(c.get_uniform_i32(p,value).unwrap(),limits);
            let mut copy = c.get_uniform_i32(p,value).unwrap();copy[0]=0;assert_eq!(c.get_uniform_i32(p,value).unwrap(),limits);
            assert_eq!(c.active_uniforms(p).unwrap()[0].kind,kind);
            let initial = &[64,128,192,255][..width];c.uniform_i32_components(value,width as u32,initial).unwrap();assert_eq!(draw(&mut c),expected);
            let mut cases = vec![(vec![],Error::InvalidValue),(vec![0;width*2],Error::InvalidOperation)];
            if width>1 {cases.push((vec![0;width-1],Error::InvalidValue));cases.push((vec![0;width+1],Error::InvalidValue));}
            for (data,error) in cases {
                assert_eq!(c.uniform_i32_components(value,width as u32,&data),Err(error));assert_eq!(c.get_uniform_i32(p,value).unwrap(),initial);assert_eq!(draw(&mut c),expected);
            }
            let wrong = if width==1 {2} else {1};
            assert_eq!(c.uniform_i32_components(value,wrong,&vec![0;wrong as usize]),Err(Error::InvalidOperation));
            assert_eq!(c.uniform_f32_components(value,width as u32,&vec![0.;width]),Err(Error::InvalidOperation));
            assert_eq!(c.get_uniform_i32(p,value).unwrap(),initial);assert_eq!(draw(&mut c),expected);
            c.uniform_i32_components(-1,0,&[]).unwrap();
        }
    }
}

#[test]
fn integer_helper_bindings_and_swizzles_keep_global_values_out_of_caller_scope() {
    for version in [100,300] {
        let source = fragment(version,"uniform highp ivec2 pair;int read_global(){return pair.x;}int choose(ivec2 value){if(value.x!=value.y){return read_global();}return value.y;}ivec3 reverse(ivec3 value){return value.zyx;}","ivec2 pair=ivec2(7);int selected=choose(ivec2(1,2));if(selected==16777217){OUTPUT=vec4(vec3(reverse(ivec3(64,128,255)))/255.0,1.0);}else{OUTPUT=vec4(1.0,0.0,0.0,1.0);}");
        let (mut c,p) = scene(version,1,None,&source);let pair = c.get_uniform_location(p,"pair").unwrap().unwrap();
        c.uniform_i32_components(pair,2,&[16_777_217,0]).unwrap();assert_eq!(draw(&mut c),[255,128,64,255]);
        c.uniform_i32_components(pair,2,&[7,0]).unwrap();assert_eq!(draw(&mut c),[255,0,0,255]);
    }
}

#[test]
fn integer_vertex_uniforms_move_real_coverage_after_explicit_conversion() {
    for version in [100,300] {
        let vertex = format!("{}{} vec2 position;uniform highp ivec2 offset;void main(){{gl_Position=vec4(position+vec2(offset)*0.5,0.0,1.0);}}",prefix(version),if version==300 {"in"} else {"attribute"});
        let source = fragment(version,"","OUTPUT=vec4(0.0,1.0,0.0,1.0);");
        let (mut c,p) = scene(version,4,Some(&vertex),&source);
        let bytes: Vec<_> = [-1f32,-1.,1.,-1.,-1.,1.].into_iter().flat_map(f32::to_le_bytes).collect();c.buffer_data(ARRAY_BUFFER,&bytes).unwrap();
        let offset = c.get_uniform_location(p,"offset").unwrap().unwrap();
        c.clear_color([0.,0.,1.,1.]);c.clear(COLOR_BUFFER_BIT).unwrap();c.draw_arrays(TRIANGLES,0,3).unwrap();
        assert_eq!(c.read_pixels(0,0,1,1).unwrap(),[0,255,0,255]);assert_eq!(c.read_pixels(3,0,1,1).unwrap(),[0,0,255,255]);
        c.uniform_i32_components(offset,2,&[2,0]).unwrap();c.clear(COLOR_BUFFER_BIT).unwrap();c.draw_arrays(TRIANGLES,0,3).unwrap();
        assert_eq!(c.read_pixels(0,0,1,1).unwrap(),[0,0,255,255]);assert_eq!(c.read_pixels(3,0,1,1).unwrap(),[0,255,0,255]);
    }
}

#[test]
fn integer_uniform_precision_matching_uses_declared_qualifiers_and_stage_defaults() {
    for version in [100,300] {
        let input = if version==300 {"in"} else {"attribute"};
        let vertex = format!("{}precision highp int;{input} vec2 position;uniform int shared;void main(){{gl_Position=vec4(position+vec2(float(shared)*0.0),0.0,1.0);}}",prefix(version));
        let source = fragment(version,"precision highp int;uniform int shared;","if(shared==7){OUTPUT=vec4(0.0,1.0,0.0,1.0);}else{OUTPUT=vec4(1.0,0.0,0.0,1.0);}");
        let (mut c,p) = scene(version,1,Some(&vertex),&source);let value = c.get_uniform_location(p,"shared").unwrap().unwrap();c.uniform_i32(value,&[7]).unwrap();assert_eq!(draw(&mut c),[0,255,0,255]);
        let mismatch = fragment(version,"uniform mediump int shared;","OUTPUT=vec4(float(shared));");
        let bad = link(&mut c,&vertex,&mismatch);assert!(!c.program_link_status(bad).unwrap());assert!(c.program_info_log(bad).unwrap().contains("precision"));
        let mismatch = fragment(version,"uniform highp ivec2 shared;","OUTPUT=vec4(vec2(shared),0.0,1.0);");
        let bad = link(&mut c,&vertex,&mismatch);assert!(!c.program_link_status(bad).unwrap());assert!(c.program_info_log(bad).unwrap().contains("type"));
        // Omitting both qualifiers resolves highp in vertex, mediump in fragment.
        let default_vertex = vertex.replace("precision highp int;","");
        let default_fragment = source.replace("precision highp int;","");
        let bad = link(&mut c,&default_vertex,&default_fragment);assert!(!c.program_link_status(bad).unwrap());assert!(c.program_info_log(bad).unwrap().contains("precision"));
    }
}

#[test]
fn glsl300_integer_literals_keep_the_full_signed_bit_pattern() {
    let source = fragment(300,"","int minimum=2147483648;int negative_one=4294967295;if(minimum<0&&negative_one==-1&&-minimum==minimum){OUTPUT=vec4(0.0,1.0,0.0,1.0);}else{OUTPUT=vec4(1.0,0.0,0.0,1.0);}");
    let (mut c,_) = scene(300,1,None,&source);assert_eq!(draw(&mut c),[0,255,0,255]);
}

#[test]
fn unsupported_integer_operations_fail_compile_without_replacing_a_live_program() {
    for version in [100,300] {
        let source = fragment(version,"","OUTPUT=vec4(0.0,1.0,0.0,1.0);");let (mut c,_) = scene(version,1,None,&source);
        for body in [
            "int value=1+2;OUTPUT=vec4(float(value));", "int value=1-2;OUTPUT=vec4(float(value));",
            "int value=1*2;OUTPUT=vec4(float(value));", "int value=1/2;OUTPUT=vec4(float(value));",
            "float value=1;OUTPUT=vec4(value);", "int value=int(1.0);OUTPUT=vec4(float(value));",
            "ivec2 value=ivec2(1.0);OUTPUT=vec4(vec2(value),0.0,1.0);", "int value=4294967296;OUTPUT=vec4(float(value));",
            "uint value=1u;OUTPUT=vec4(1.0);", "int value=min(1,2);OUTPUT=vec4(float(value));",
            "ivec2 value=ivec2(1,2);if(value<value){OUTPUT=vec4(1.0);}else{OUTPUT=vec4(0.0);}",
            "ivec2 value=ivec2(1,2);OUTPUT=vec4(float(value.z));",
        ] {
            let shader = c.create_shader(FRAGMENT_SHADER).unwrap();c.shader_source(shader,&fragment(version,"",body)).unwrap();c.compile_shader(shader).unwrap();
            assert!(!c.shader_compile_status(shader).unwrap(),"accepted {body}");assert!(!c.shader_info_log(shader).unwrap().is_empty());assert_eq!(draw(&mut c),[0,255,0,255]);
        }
    }
}
