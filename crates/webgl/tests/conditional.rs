use rabbithole_webgl::{gl::*, Context};

#[test]
fn typed_ternary_selections_in_vertex_and_fragment_produce_real_pixels() {
    for version in [100, 300] {
        let mut c = if version == 300 { Context::new_webgl2(1, 1).unwrap() } else { Context::new(1, 1).unwrap() };
        let (prefix, input, declaration, output) = if version == 300 { ("#version 300 es\n", "in", "out vec4 pixel;", "pixel") } else { ("", "attribute", "", "gl_FragColor") };
        let sources = [
            (VERTEX_SHADER, format!("{prefix}{input} vec2 position;void main(){{gl_Position=true?vec4(position,0.0,1.0):vec4(0.0);}}")),
            (FRAGMENT_SHADER, format!("{prefix}precision highp float;uniform bool selected;{declaration}void main(){{{output}=selected?vec4(1.0,0.0,0.0,1.0):vec4(0.0,1.0,0.0,1.0);}}")),
        ];
        let program = c.create_program();
        for (kind, source) in sources {
            let shader = c.create_shader(kind).unwrap();
            c.shader_source(shader, &source).unwrap(); c.compile_shader(shader).unwrap();
            assert!(c.shader_compile_status(shader).unwrap(), "{}", c.shader_info_log(shader).unwrap());
            c.attach_shader(program, shader).unwrap();
        }
        c.link_program(program).unwrap();
        assert!(c.program_link_status(program).unwrap(), "{}", c.program_info_log(program).unwrap());
        c.use_program(program).unwrap();
        let buffer = c.create_buffer(); c.bind_buffer(ARRAY_BUFFER, buffer).unwrap();
        let bytes: Vec<_> = [-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect();
        c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
        let location = c.get_attrib_location(program, "position").unwrap() as u32;
        c.vertex_attrib_pointer(location, 2, FLOAT, false, 0, 0).unwrap(); c.enable_vertex_attrib_array(location, true).unwrap();
        let selected = c.get_uniform_location(program, "selected").unwrap();
        for (value, expected) in [(0, [0,255,0,255]), (1, [255,0,0,255]), (0, [0,255,0,255])] {
            c.uniform_i32(selected, 1, &[value]).unwrap();
            c.draw_arrays(TRIANGLES, 0, 3).unwrap();
            assert_eq!(c.read_pixels(0,0,1,1).unwrap(), expected, "{version}");
        }
    }
}
