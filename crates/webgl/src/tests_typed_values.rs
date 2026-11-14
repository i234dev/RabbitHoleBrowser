use super::*;

#[test]
fn malformed_uniform_payload_is_rejected_before_any_color_or_depth_write() {
    for version in [1, 2] {
        let mut c = if version == 2 { Context::new_webgl2(1, 1) } else { Context::new(1, 1) }.unwrap();
        let prefix = if version == 2 { "#version 300 es\n" } else { "" };
        let input = if version == 2 { "in" } else { "attribute" };
        let output = if version == 2 { "pixel" } else { "gl_FragColor" };
        let vertex = format!("{prefix}{input} vec2 position;void main(){{gl_Position=vec4(position,0.0,1.0);}}");
        let fragment = format!("{prefix}precision mediump float;{}uniform vec4 color;void main(){{{output}=color;}}", if version == 2 { "out vec4 pixel;" } else { "" });
        let p = c.create_program();
        for (kind, source) in [(gl::VERTEX_SHADER, vertex), (gl::FRAGMENT_SHADER, fragment)] {
            let s = c.create_shader(kind).unwrap();c.shader_source(s, &source).unwrap();c.compile_shader(s).unwrap();
            assert!(c.shader_compile_status(s).unwrap(), "{}", c.shader_info_log(s).unwrap());
            c.attach_shader(p, s).unwrap();
        }
        c.link_program(p).unwrap();assert!(c.program_link_status(p).unwrap());c.use_program(p).unwrap();
        let b = c.create_buffer();c.bind_buffer(gl::ARRAY_BUFFER, b).unwrap();
        let bytes: Vec<_> = [-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect();
        c.buffer_data(gl::ARRAY_BUFFER, &bytes).unwrap();
        let a = c.get_attrib_location(p, "position").unwrap() as u32;
        c.vertex_attrib_pointer(a, 2, gl::FLOAT, false, 0, 0).unwrap();c.enable_vertex_attrib_array(a, true).unwrap();
        let location = c.get_uniform_location(p, "color").unwrap().unwrap();
        c.uniform_f32_components(location, 4, &[1.,0.,0.,1.]).unwrap();c.draw_arrays(gl::TRIANGLES, 0, 3).unwrap();
        assert_eq!(c.rgba, [255,0,0,255]);
        c.enable_depth_test(true);c.clear_color([0.,0.,1.,1.]);c.clear(gl::COLOR_BUFFER_BIT | gl::DEPTH_BUFFER_BIT).unwrap();
        let before_color = c.rgba.clone();let before_depth = c.depth.clone();
        for bad in [Value::Integers(Ty::Vec4, [0;4]), Value::Floats(Ty::Float, [0.;16])] {
            c.programs.get_mut(&p).unwrap().executable.as_mut().unwrap().uniforms[0].value = bad;
            assert_eq!(c.draw_arrays(gl::TRIANGLES, 0, 3), Err(Error::InvalidOperation));
            assert_eq!(c.rgba, before_color);assert_eq!(c.depth, before_depth);
        }
    }
}

fn stage_scene(version: u32, vertex: &str, fragment: &str) -> (Context, u32) {
    let mut c = if version == 2 { Context::new_webgl2(1, 1) } else { Context::new(1, 1) }.unwrap();
    let p = c.create_program();
    for (kind, source) in [(gl::VERTEX_SHADER, vertex), (gl::FRAGMENT_SHADER, fragment)] {
        let s = c.create_shader(kind).unwrap();c.shader_source(s, source).unwrap();c.compile_shader(s).unwrap();
        assert!(c.shader_compile_status(s).unwrap(), "{}", c.shader_info_log(s).unwrap());c.attach_shader(p, s).unwrap();
    }
    c.link_program(p).unwrap();assert!(c.program_link_status(p).unwrap(), "{}", c.program_info_log(p).unwrap());c.use_program(p).unwrap();
    let b = c.create_buffer();c.bind_buffer(gl::ARRAY_BUFFER, b).unwrap();
    let bytes: Vec<_> = [-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect();c.buffer_data(gl::ARRAY_BUFFER, &bytes).unwrap();
    let position = c.get_attrib_location(p, "position").unwrap() as u32;
    c.vertex_attrib_pointer(position, 2, gl::FLOAT, false, 0, 0).unwrap();c.enable_vertex_attrib_array(position, true).unwrap();
    (c, p)
}

#[test]
fn uniform_validation_keeps_vertex_attribute_and_fragment_uniform_names_separate() {
    for version in [1, 2] {
        let prefix = if version == 2 { "#version 300 es\n" } else { "" };
        let input = if version == 2 { "in" } else { "attribute" };
        let output = if version == 2 { "pixel" } else { "gl_FragColor" };
        let vertex = format!("{prefix}{input} vec2 position;{input} vec3 y;void main(){{gl_Position=vec4(position+y.xy*0.0,0.0,1.0);}}");
        let fragment = format!("{prefix}precision mediump float;{}uniform float y;void main(){{{output}=vec4(y,0.0,0.0,1.0);}}", if version == 2 { "out vec4 pixel;" } else { "" });
        let (mut c, p) = stage_scene(version, &vertex, &fragment);
        assert!(c.get_attrib_location(p, "y").unwrap() >= 0);
        let y = c.get_uniform_location(p, "y").unwrap().unwrap();
        c.uniform_f32_components(y, 1, &[0.5]).unwrap();c.draw_arrays(gl::TRIANGLES, 0, 3).unwrap();
        assert_eq!(c.read_pixels(0, 0, 1, 1).unwrap(), [128,0,0,255]);
    }
}

#[test]
fn vertex_uniform_does_not_initialize_a_same_named_fragment_output() {
    let vertex = "#version 300 es\nin vec2 position;uniform float pixel;void main(){gl_Position=vec4(position+vec2(pixel*0.0),0.0,1.0);}";
    let fragment = "#version 300 es\nprecision mediump float;out vec4 pixel;void main(){pixel=vec4(0.0,0.5,0.0,1.0);}";
    let (mut c, p) = stage_scene(2, vertex, fragment);
    let pixel = c.get_uniform_location(p, "pixel").unwrap().unwrap();
    c.uniform_f32_components(pixel, 1, &[0.75]).unwrap();c.draw_arrays(gl::TRIANGLES, 0, 3).unwrap();
    assert_eq!(c.get_uniform(p, pixel).unwrap(), [0.75]);
    assert_eq!(c.read_pixels(0, 0, 1, 1).unwrap(), [0,128,0,255]);
}
