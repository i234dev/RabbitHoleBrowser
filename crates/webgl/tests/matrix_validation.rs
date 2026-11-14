use rabbithole_webgl::{gl::*, Context, Error};

fn scene(version: u32) -> (Context, u32, i32, i32) {
    let mut c = if version == 300 { Context::new_webgl2(1, 1) } else { Context::new(1, 1) }.unwrap();
    let prefix = if version == 300 { "#version 300 es\n" } else { "" };
    let attribute = if version == 300 { "in" } else { "attribute" };
    let output = if version == 300 { "pixel" } else { "gl_FragColor" };
    let vertex = format!("{prefix}{attribute} vec2 position;void main(){{gl_Position=vec4(position,0.0,1.0);}}");
    let fragment = format!("{prefix}precision mediump float;{}uniform mat4 matrix;uniform vec4 color;void main(){{{output}=matrix*color;}}", if version == 300 { "out vec4 pixel;" } else { "" });
    let p = c.create_program();
    for (kind, source) in [(VERTEX_SHADER, vertex), (FRAGMENT_SHADER, fragment)] {
        let s = c.create_shader(kind).unwrap();
        c.shader_source(s, &source).unwrap();
        c.compile_shader(s).unwrap();
        assert!(c.shader_compile_status(s).unwrap(), "{}", c.shader_info_log(s).unwrap());
        c.attach_shader(p, s).unwrap();
    }
    c.link_program(p).unwrap();
    assert!(c.program_link_status(p).unwrap(), "{}", c.program_info_log(p).unwrap());
    c.use_program(p).unwrap();
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    let bytes: Vec<_> = [-1f32, -1., 3., -1., -1., 3.].into_iter().flat_map(f32::to_le_bytes).collect();
    c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
    let a = c.get_attrib_location(p, "position").unwrap() as u32;
    c.vertex_attrib_pointer(a, 2, FLOAT, false, 0, 0).unwrap();
    c.enable_vertex_attrib_array(a, true).unwrap();
    let matrix = c.get_uniform_location(p, "matrix").unwrap().unwrap();
    let color = c.get_uniform_location(p, "color").unwrap().unwrap();
    c.uniform_f32(color, &[1., 0., 0., 1.]).unwrap();
    (c, p, matrix, color)
}

#[test]
fn matrix4_invalid_lengths_and_transpose_preserve_readback_and_pixels() {
    for version in [100, 300] {
        let (mut c, p, matrix, color) = scene(version);
        let good = [0.25, 0.5, 0.75, 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.];
        c.uniform_matrix4fv(matrix, false, &good).unwrap();
        for (transpose, values, expected) in [
            (false, vec![], Error::InvalidValue),
            (false, vec![0.; 15], Error::InvalidValue),
            (false, vec![0.; 17], Error::InvalidValue),
            (false, vec![0.; 32], Error::InvalidOperation),
            (true, vec![0.; 16], Error::InvalidValue),
        ] {
            assert_eq!(c.uniform_matrix4fv(matrix, transpose, &values), Err(expected));
            assert_eq!(c.get_uniform(p, matrix).unwrap(), good);
            c.draw_arrays(TRIANGLES, 0, 3).unwrap();
            assert_eq!(c.read_pixels(0, 0, 1, 1).unwrap(), [64, 128, 191, 255]);
        }
        assert_eq!(c.uniform_matrix4fv(color, false, &good), Err(Error::InvalidOperation));
        assert_eq!(c.get_uniform(p, color).unwrap(), [1., 0., 0., 1.]);
        c.use_program(0).unwrap();
        assert_eq!(c.uniform_matrix4fv(matrix, false, &good), Err(Error::InvalidOperation));
    }
}

#[test]
fn matrix4_null_backend_location_ignores_transpose_length_and_current_program() {
    let mut c = Context::new(1, 1).unwrap();
    for transpose in [false, true] {
        c.uniform_matrix4fv(-1, transpose, &[]).unwrap();
        c.uniform_matrix4fv(-1, transpose, &[0.; 17]).unwrap();
    }
}
