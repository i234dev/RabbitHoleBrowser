use rabbithole_webgl::{gl::*, Context, Error};

fn scene(version: u32, ty: &str, body: &str) -> (Context, u32, i32) {
    let mut c = if version == 2 { Context::new_webgl2(1, 1) } else { Context::new(1, 1) }.unwrap();
    let prefix = if version == 2 { "#version 300 es\n" } else { "" };
    let input = if version == 2 { "in" } else { "attribute" };
    let output = if version == 2 { "pixel" } else { "gl_FragColor" };
    let vertex = format!("{prefix}{input} vec2 position;void main(){{gl_Position=vec4(position,0.0,1.0);}}");
    let fragment = format!("{prefix}precision mediump float;{}uniform {ty} value;void main(){{{}}}", if version == 2 { "out vec4 pixel;" } else { "" }, body.replace("OUTPUT", output));
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
    let location = c.get_uniform_location(p, "value").unwrap().unwrap();
    (c, p, location)
}

fn pixel(c: &mut Context) -> Vec<u8> {
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    c.read_pixels(0, 0, 1, 1).unwrap()
}

#[test]
fn float_vector_lengths_distinguish_invalid_value_from_non_array_count_atomically() {
    for version in [1, 2] {
        for (width, ty, body, expected) in [
            (1, "float", "OUTPUT=vec4(value);", [64, 64, 64, 64]),
            (2, "vec2", "OUTPUT=vec4(value,0.0,1.0);", [64, 128, 0, 255]),
            (3, "vec3", "OUTPUT=vec4(value,1.0);", [64, 128, 191, 255]),
            (4, "vec4", "OUTPUT=value;", [64, 128, 191, 255]),
        ] {
            let (mut c, p, loc) = scene(version, ty, body);
            let initial = &[0.25, 0.5, 0.75, 1.][..width];
            c.uniform_f32_components(loc, width as u32, initial).unwrap();
            assert_eq!(pixel(&mut c), expected);
            let mut cases = vec![(vec![], Error::InvalidValue), (vec![0.; width * 2], Error::InvalidOperation)];
            if width > 1 {
                cases.push((vec![0.; width - 1], Error::InvalidValue));
                cases.push((vec![0.; width + 1], Error::InvalidValue));
            }
            for (values, error) in cases {
                assert_eq!(c.uniform_f32_components(loc, width as u32, &values), Err(error));
                assert_eq!(c.get_uniform(p, loc).unwrap(), initial);
                assert_eq!(pixel(&mut c), expected);
            }
            c.uniform_f32_components(-1, width as u32, &[]).unwrap();
        }
    }
}

#[test]
fn empty_integer_uniform_is_invalid_value_without_changing_bool_or_pixels() {
    for version in [1, 2] {
        let (mut c, p, loc) = scene(version, "bool", "if(value){OUTPUT=vec4(1.0,0.0,0.0,1.0);}else{OUTPUT=vec4(0.0,1.0,0.0,1.0);}");
        c.uniform_i32(loc, &[1]).unwrap();
        assert_eq!(pixel(&mut c), [255, 0, 0, 255]);
        for (values, error) in [(&[][..], Error::InvalidValue), (&[0, 0][..], Error::InvalidOperation)] {
            assert_eq!(c.uniform_i32(loc, values), Err(error));
            assert_eq!(c.get_uniform(p, loc).unwrap(), [1.]);
            assert_eq!(pixel(&mut c), [255, 0, 0, 255]);
        }
        c.uniform_i32(-1, &[]).unwrap();
    }
}

fn matrix(c: &mut Context, loc: i32, dim: usize, transpose: bool, values: &[f32]) -> Result<(), Error> {
    match dim {
        2 => c.uniform_matrix2fv(loc, transpose, values),
        3 => c.uniform_matrix3fv(loc, transpose, values),
        4 => c.uniform_matrix4fv(loc, transpose, values),
        _ => unreachable!(),
    }
}

#[test]
fn square_matrix_lengths_and_transpose_preserve_uniforms_and_pixels() {
    for version in [1, 2] {
        for (dim, ty, body, expected) in [
            (2, "mat2", "OUTPUT=vec4(value*vec2(1.0,0.0),0.0,1.0);", [64, 128, 0, 255]),
            (3, "mat3", "OUTPUT=vec4(value*vec3(1.0,0.0,0.0),1.0);", [64, 128, 191, 255]),
            (4, "mat4", "OUTPUT=value*vec4(1.0,0.0,0.0,0.0);", [64, 128, 191, 255]),
        ] {
            let (mut c, p, loc) = scene(version, ty, body);
            let mut initial = vec![0.; dim * dim];
            initial[..dim].copy_from_slice(&[0.25, 0.5, 0.75, 1.][..dim]);
            matrix(&mut c, loc, dim, false, &initial).unwrap();
            assert_eq!(pixel(&mut c), expected);
            for (transpose, values, error) in [
                (false, vec![], Error::InvalidValue),
                (false, vec![0.; dim * dim - 1], Error::InvalidValue),
                (false, vec![0.; dim * dim + 1], Error::InvalidValue),
                (false, vec![0.; dim * dim * 2], Error::InvalidOperation),
                (true, initial.clone(), Error::InvalidValue),
            ] {
                assert_eq!(matrix(&mut c, loc, dim, transpose, &values), Err(error));
                assert_eq!(c.get_uniform(p, loc).unwrap(), initial);
                assert_eq!(pixel(&mut c), expected);
            }
            matrix(&mut c, -1, dim, true, &[]).unwrap();
        }
    }
}
