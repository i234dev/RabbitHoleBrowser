use rabbithole_webgl::{gl::*, Context, Error};

const TEXELS: [u8; 16] = [
    255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
];

fn setup(width: u32, height: u32, varying: bool) -> (Context, u32) {
    let mut c = Context::new(width, height).unwrap();
    let p = c.create_program();
    let fragment = if varying {
        "precision mediump float;uniform sampler2D tex;varying vec2 vUv;void main(){gl_FragColor=texture2D(tex,vUv);}"
    } else {
        "precision mediump float;uniform sampler2D tex;uniform vec2 uv;void main(){gl_FragColor=texture2D(tex,uv);}"
    };
    for (kind, source) in [
        (VERTEX_SHADER, "attribute vec2 p;varying vec2 vUv;void main(){gl_Position=vec4(p,0.0,1.0);vUv=p*0.5+0.5;}"),
        (FRAGMENT_SHADER, fragment),
    ] {
        let s = c.create_shader(kind).unwrap();
        c.shader_source(s, source).unwrap();
        c.compile_shader(s).unwrap();
        assert!(c.shader_compile_status(s).unwrap(), "{}", c.shader_info_log(s).unwrap());
        c.attach_shader(p, s).unwrap();
    }
    c.link_program(p).unwrap();
    assert!(
        c.program_link_status(p).unwrap(),
        "{}",
        c.program_info_log(p).unwrap()
    );
    c.use_program(p).unwrap();
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    let vertices = [-1f32, -1., 3., -1., -1., 3.];
    let bytes: Vec<u8> = vertices.iter().flat_map(|v| v.to_le_bytes()).collect();
    c.buffer_data(ARRAY_BUFFER, &bytes).unwrap();
    let a = c.get_attrib_location(p, "p").unwrap() as u32;
    c.vertex_attrib_pointer(a, 2, FLOAT, false, 0, 0).unwrap();
    c.enable_vertex_attrib_array(a, true).unwrap();
    let t = c.create_texture();
    c.bind_texture(TEXTURE_2D, t).unwrap();
    for name in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
        c.tex_parameter_i(TEXTURE_2D, name, LINEAR as i32).unwrap();
    }
    for name in [TEXTURE_WRAP_S, TEXTURE_WRAP_T] {
        c.tex_parameter_i(TEXTURE_2D, name, CLAMP_TO_EDGE as i32)
            .unwrap();
    }
    upload(&mut c, 2, 2, &TEXELS);
    (c, p)
}
fn upload(c: &mut Context, w: u32, h: u32, pixels: &[u8]) {
    c.tex_image_2d(
        TEXTURE_2D,
        0,
        RGBA,
        w,
        h,
        0,
        RGBA,
        UNSIGNED_BYTE,
        Some(pixels),
    )
    .unwrap();
}
fn pixel(c: &Context, x: i32, y: i32) -> [u8; 4] {
    c.read_pixels(x, y, 1, 1).unwrap().try_into().unwrap()
}
fn sample(c: &mut Context, p: u32, uv: [f32; 2]) -> [u8; 4] {
    let location = c.get_uniform_location(p, "uv").unwrap().unwrap();
    c.uniform_f32(location, &uv).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    pixel(c, 0, 0)
}

#[test]
fn linear_centers_boundaries_and_four_texel_weights() {
    let (mut c, p) = setup(1, 1, false);
    for (uv, expected) in [
        ([0.25, 0.25], [255, 0, 0, 255]),
        ([0.75, 0.25], [0, 255, 0, 255]),
        ([0.25, 0.75], [0, 0, 255, 255]),
        ([0.75, 0.75], [255; 4]),
        ([0.5, 0.25], [128, 128, 0, 255]),
        ([0.5, 0.5], [128, 128, 128, 255]),
        ([0.375, 0.375], [159, 64, 64, 255]),
        ([0., 0.], [255, 0, 0, 255]),
        ([1., 1.], [255; 4]),
        ([-f32::MAX, f32::MAX], [0, 0, 255, 255]),
    ] {
        assert_eq!(sample(&mut c, p, uv), expected, "uv={uv:?}");
    }
}

#[test]
fn linear_fragment_pixel_centers_preserve_readback_orientation() {
    let (mut c, _) = setup(3, 3, true);
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 2, 0), [0, 255, 0, 255]);
    assert_eq!(pixel(&c, 0, 2), [0, 0, 255, 255]);
    assert_eq!(pixel(&c, 2, 2), [255; 4]);
    assert_eq!(pixel(&c, 1, 1), [128, 128, 128, 255]);
    assert_eq!(pixel(&c, 1, 0), [128, 128, 0, 255]);
}

#[test]
fn linear_repeat_and_mirror_filter_across_negative_wrap_boundaries() {
    let (mut c, p) = setup(1, 1, false);
    for (mode, u, expected) in [
        (REPEAT, 0., [128, 128, 0, 255]),
        (REPEAT, 1., [128, 128, 0, 255]),
        (REPEAT, -1., [128, 128, 0, 255]),
        (REPEAT, -0.125, [64, 191, 0, 255]),
        (REPEAT, 1.125, [191, 64, 0, 255]),
        (MIRRORED_REPEAT, 0., [255, 0, 0, 255]),
        (MIRRORED_REPEAT, -0.125, [255, 0, 0, 255]),
        (MIRRORED_REPEAT, 1.125, [0, 255, 0, 255]),
        (MIRRORED_REPEAT, -0.5, [128, 128, 0, 255]),
        (MIRRORED_REPEAT, -1., [0, 255, 0, 255]),
        (MIRRORED_REPEAT, 2., [255, 0, 0, 255]),
    ] {
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_WRAP_S, mode as i32)
            .unwrap();
        assert_eq!(
            sample(&mut c, p, [u, 0.25]),
            expected,
            "mode={mode:x}, u={u}"
        );
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_WRAP_T, mode as i32)
            .unwrap();
        let result = sample(&mut c, p, [0.25, u]);
        // The second axis has blue rather than green as its second texel.
        assert_eq!(
            result,
            [expected[0], 0, expected[1], expected[3]],
            "mode={mode:x}, v={u}"
        );
        c.tex_parameter_i(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE as i32)
            .unwrap();
    }
}

#[test]
fn linear_single_texel_axes_and_alpha_are_interpolated_without_premultiplication() {
    let (mut c, p) = setup(1, 1, false);
    let data = [0, 0, 0, 0, 255, 128, 64, 128];
    for (w, h) in [(1, 2), (2, 1)] {
        upload(&mut c, w, h, &data);
        for wrap in [CLAMP_TO_EDGE, REPEAT, MIRRORED_REPEAT] {
            for name in [TEXTURE_WRAP_S, TEXTURE_WRAP_T] {
                c.tex_parameter_i(TEXTURE_2D, name, wrap as i32).unwrap();
            }
            let uv = if w == 1 { [-3.25, 0.5] } else { [0.5, -3.25] };
            assert_eq!(sample(&mut c, p, uv), [128, 64, 32, 64]);
        }
    }
    upload(&mut c, 1, 1, &[7, 31, 83, 129]);
    assert_eq!(sample(&mut c, p, [-100., 100.]), [7, 31, 83, 129]);
}

#[test]
fn linear_npot_requires_clamp_on_both_axes() {
    let (mut c, p) = setup(1, 1, false);
    upload(
        &mut c,
        1,
        3,
        &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
    );
    assert_eq!(sample(&mut c, p, [0.5, 0.5]), [0, 255, 0, 255]);
    for name in [TEXTURE_WRAP_S, TEXTURE_WRAP_T] {
        for wrap in [REPEAT, MIRRORED_REPEAT] {
            c.tex_parameter_i(TEXTURE_2D, name, wrap as i32).unwrap();
            assert_eq!(sample(&mut c, p, [0.5, 0.5]), [0, 0, 0, 255]);
        }
        c.tex_parameter_i(TEXTURE_2D, name, CLAMP_TO_EDGE as i32)
            .unwrap();
    }
    assert_eq!(sample(&mut c, p, [0.5, 0.5]), [0, 255, 0, 255]);
}

#[test]
fn linear_mixed_filters_and_mipmaps_fail_before_touching_pixels() {
    let (mut c, _) = setup(1, 1, false);
    for name in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
        c.clear_color([0., 0., 1., 1.]);
        c.clear(COLOR_BUFFER_BIT).unwrap();
        c.tex_parameter_i(TEXTURE_2D, name, NEAREST as i32).unwrap();
        assert_eq!(c.draw_arrays(TRIANGLES, 0, 3), Err(Error::InvalidOperation));
        assert_eq!(pixel(&c, 0, 0), [0, 0, 255, 255]);
        c.tex_parameter_i(TEXTURE_2D, name, LINEAR as i32).unwrap();
        for value in [0, -1, NEAREST_MIPMAP_LINEAR as i32] {
            assert_eq!(
                c.tex_parameter_i(TEXTURE_2D, name, value),
                Err(Error::InvalidEnum)
            );
            assert_eq!(c.texture_parameter(TEXTURE_2D, name), Ok(LINEAR as i32));
        }
    }
}
