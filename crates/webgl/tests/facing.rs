use rabbithole_webgl::{gl::*, Context, Error};

fn scene(version: u32) -> (Context, u32) {
    let mut c = if version == 300 { Context::new_webgl2(4, 4).unwrap() } else { Context::new(4, 4).unwrap() };
    let (prefix, input, declaration, output) = if version == 300 { ("#version 300 es\n", "in", "out vec4 pixel;", "pixel") } else { ("", "attribute", "", "gl_FragColor") };
    let program = c.create_program();
    for (kind, source) in [
        (VERTEX_SHADER, format!("{prefix}{input} vec2 position;void main(){{gl_Position=vec4(position,0.0,1.0);}}")),
        (FRAGMENT_SHADER, format!("{prefix}precision highp float;{declaration}bool facing(){{return gl_FrontFacing;}}void main(){{{output}=facing()?vec4(1.0,0.0,0.0,1.0):vec4(0.0,1.0,0.0,1.0);}}")),
    ] {
        let shader = c.create_shader(kind).unwrap();
        c.shader_source(shader, &source).unwrap(); c.compile_shader(shader).unwrap();
        assert!(c.shader_compile_status(shader).unwrap(), "{}", c.shader_info_log(shader).unwrap());
        c.attach_shader(program, shader).unwrap();
    }
    c.link_program(program).unwrap(); assert!(c.program_link_status(program).unwrap(), "{}", c.program_info_log(program).unwrap()); c.use_program(program).unwrap();
    let buffer = c.create_buffer(); c.bind_buffer(ARRAY_BUFFER, buffer).unwrap();
    let location = c.get_attrib_location(program, "position").unwrap() as u32;
    c.vertex_attrib_pointer(location, 2, FLOAT, false, 0, 0).unwrap(); c.enable_vertex_attrib_array(location, true).unwrap();
    (c, program)
}

#[test]
fn original_facing_survives_clipping_canonicalization_and_strip_parity_in_both_versions() {
    for version in [100, 300] {
        let (mut c, _) = scene(version);
        for (mode, positions, original_ccw) in [
            (TRIANGLES, vec![-1f32,-1.,3.,-1.,-1.,3.], true),
            (TRIANGLES, vec![-1.,-1.,-1.,3.,3.,-1.], false),
            (TRIANGLE_STRIP, vec![-1.,-1.,1.,-1.,-1.,1.,1.,1.], true),
            (TRIANGLE_STRIP, vec![1.,-1.,-1.,-1.,1.,1.,-1.,1.], false),
        ] {
            let count = positions.len() as u32 / 2;
            let bytes: Vec<_> = positions.into_iter().flat_map(f32::to_le_bytes).collect();
            c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
            for winding in [CCW, CW] {
                c.front_face(winding).unwrap();
                // An invalid state change must leave the previous winding intact.
                assert_eq!(c.front_face(0), Err(Error::InvalidEnum));
                c.draw_arrays(mode, 0, count).unwrap();
                let expected = if original_ccw == (winding == CCW) { [255,0,0,255] } else { [0,255,0,255] };
                assert!(c.read_pixels(0,0,4,4).unwrap().chunks_exact(4).all(|p| p == expected), "version {version}, mode {mode}, winding {winding}");
            }
        }
    }
}

#[test]
fn facing_is_a_read_only_fragment_bool_and_not_a_vertex_input() {
    for (kind, source) in [
        (VERTEX_SHADER, "void main(){if(gl_FrontFacing){gl_Position=vec4(1.0);}else{gl_Position=vec4(0.0);}}"),
        (FRAGMENT_SHADER, "precision highp float;void main(){gl_FrontFacing=true;gl_FragColor=vec4(1.0);}"),
    ] {
        let mut c = Context::new(1,1).unwrap();
        let shader = c.create_shader(kind).unwrap(); c.shader_source(shader,source).unwrap(); c.compile_shader(shader).unwrap();
        assert!(!c.shader_compile_status(shader).unwrap());
    }
}
