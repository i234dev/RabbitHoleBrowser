use rabbithole_webgl::{gl::*, Context, Error};

fn scene() -> (Context, i32, i32) {
    let mut ctx = Context::new(4, 4).unwrap();
    let program = ctx.create_program();
    for (kind, source) in [
        (VERTEX_SHADER, "attribute vec2 position; uniform float z; void main(){gl_Position=vec4(position,z,1.0);}"),
        (FRAGMENT_SHADER, "precision mediump float; uniform vec4 color; void main(){gl_FragColor=color;}"),
    ] {
        let shader = ctx.create_shader(kind).unwrap();
        ctx.shader_source(shader, source).unwrap();
        ctx.compile_shader(shader).unwrap();
        assert!(ctx.shader_compile_status(shader).unwrap(), "{}", ctx.shader_info_log(shader).unwrap());
        ctx.attach_shader(program, shader).unwrap();
    }
    ctx.link_program(program).unwrap();
    assert!(ctx.program_link_status(program).unwrap());
    ctx.use_program(program).unwrap();
    let buffer = ctx.create_buffer();
    ctx.bind_buffer(ARRAY_BUFFER, buffer).unwrap();
    let vertices = [-1f32, -1., 3., -1., -1., 3.];
    ctx.buffer_data(
        ARRAY_BUFFER,
        &vertices
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let attribute = ctx.get_attrib_location(program, "position").unwrap() as u32;
    ctx.vertex_attrib_pointer(attribute, 2, FLOAT, false, 0, 0)
        .unwrap();
    ctx.enable_vertex_attrib_array(attribute, true).unwrap();
    let color = ctx.get_uniform_location(program, "color").unwrap().unwrap();
    let z = ctx.get_uniform_location(program, "z").unwrap().unwrap();
    (ctx, color, z)
}
fn pixel(ctx: &Context) -> [u8; 4] {
    ctx.read_pixels(1, 1, 1, 1).unwrap().try_into().unwrap()
}
fn draw(ctx: &mut Context, color: i32, value: [f32; 4]) {
    ctx.uniform_f32(color, &value).unwrap();
    ctx.draw_arrays(TRIANGLES, 0, 3).unwrap();
}
fn near(actual: [u8; 4], expected: [f32; 4]) {
    for i in 0..4 {
        let want = (expected[i].clamp(0., 1.) * 255.).round() as i32;
        assert!(
            (actual[i] as i32 - want).abs() <= 1,
            "actual {actual:?}, expected {expected:?}"
        );
    }
}
#[test]
fn all_webgl1_factors_produce_actual_source_and_destination_pixels() {
    let (mut ctx, color, _) = scene();
    let source = [0.8, 0.4, 0.2, 0.6];
    let destination = [0.2, 0.4, 0.6, 0.8];
    ctx.blend_color([0.3, 0.5, 0.7, 0.25]);
    ctx.enable_blend(true);
    // Independent factor vectors for these known source/destination/constant colors.
    for (factor, weights) in [
        (ZERO, [0.; 4]),
        (ONE, [1.; 4]),
        (SRC_COLOR, source),
        (ONE_MINUS_SRC_COLOR, [0.2, 0.6, 0.8, 0.4]),
        (DST_COLOR, destination),
        (ONE_MINUS_DST_COLOR, [0.8, 0.6, 0.4, 0.2]),
        (SRC_ALPHA, [0.6; 4]),
        (ONE_MINUS_SRC_ALPHA, [0.4; 4]),
        (DST_ALPHA, [0.8; 4]),
        (ONE_MINUS_DST_ALPHA, [0.2; 4]),
        (CONSTANT_COLOR, [0.3, 0.5, 0.7, 0.25]),
        (ONE_MINUS_CONSTANT_COLOR, [0.7, 0.5, 0.3, 0.75]),
        (CONSTANT_ALPHA, [0.25; 4]),
        (ONE_MINUS_CONSTANT_ALPHA, [0.75; 4]),
        (SRC_ALPHA_SATURATE, [0.2, 0.2, 0.2, 1.]),
    ] {
        ctx.clear_color(destination);
        ctx.clear(COLOR_BUFFER_BIT).unwrap();
        ctx.blend_func(factor, ZERO).unwrap();
        draw(&mut ctx, color, source);
        near(pixel(&ctx), std::array::from_fn(|k| source[k] * weights[k]));
        if factor != SRC_ALPHA_SATURATE {
            ctx.clear(COLOR_BUFFER_BIT).unwrap();
            ctx.blend_func(ZERO, factor).unwrap();
            draw(&mut ctx, color, source);
            near(
                pixel(&ctx),
                std::array::from_fn(|k| destination[k] * weights[k]),
            );
        }
    }
}
#[test]
fn separate_equations_and_factors_clamp_and_preserve_raw_alpha() {
    let (mut ctx, color, _) = scene();
    ctx.enable_blend(true);
    ctx.blend_func(ONE, ONE).unwrap();
    ctx.clear_color([0.2, 0.4, 0.6, 0.8]);
    for (rgb, rgb_expected) in [
        (FUNC_ADD, [1., 0.8, 0.8]),
        (FUNC_SUBTRACT, [0.6, 0., 0.]),
        (FUNC_REVERSE_SUBTRACT, [0., 0., 0.4]),
    ] {
        for (alpha, alpha_expected) in [
            (FUNC_ADD, 1.),
            (FUNC_SUBTRACT, 0.),
            (FUNC_REVERSE_SUBTRACT, 0.2),
        ] {
            ctx.clear(COLOR_BUFFER_BIT).unwrap();
            ctx.blend_equation_separate(rgb, alpha).unwrap();
            draw(&mut ctx, color, [0.8, 0.4, 0.2, 0.6]);
            near(
                pixel(&ctx),
                [
                    rgb_expected[0],
                    rgb_expected[1],
                    rgb_expected[2],
                    alpha_expected,
                ],
            );
        }
    }
    ctx.blend_equation(FUNC_ADD).unwrap();
    ctx.blend_func_separate(SRC_ALPHA, ONE_MINUS_SRC_ALPHA, ONE, ONE_MINUS_SRC_ALPHA)
        .unwrap();
    ctx.clear_color([0., 0., 1., 1.]);
    ctx.clear(COLOR_BUFFER_BIT).unwrap();
    draw(&mut ctx, color, [1., 0., 0., 0.5]);
    near(pixel(&ctx), [0.5, 0., 0.5, 1.]);
    ctx.enable_blend(false);
    draw(&mut ctx, color, [2., -1., 0.4, 0.25]);
    near(pixel(&ctx), [1., 0., 0.4, 0.25]);
}
#[test]
fn invalid_blend_state_is_atomic_and_constant_restriction_only_uses_rgb_pair() {
    let mut ctx = Context::new(1, 1).unwrap();
    assert!(!ctx.blend_enabled());
    assert_eq!(ctx.blend_functions(), [ONE, ZERO, ONE, ZERO]);
    assert_eq!(ctx.blend_equations(), [FUNC_ADD; 2]);
    assert_eq!(ctx.blend_constant(), [0.; 4]);
    ctx.blend_color([-1., 2., 0.25, f32::NAN]);
    assert_eq!(ctx.blend_constant(), [0., 1., 0.25, 0.]);
    for color in [CONSTANT_COLOR, ONE_MINUS_CONSTANT_COLOR] {
        for alpha in [CONSTANT_ALPHA, ONE_MINUS_CONSTANT_ALPHA] {
            assert_eq!(ctx.blend_func(color, alpha), Err(Error::InvalidOperation));
            assert_eq!(ctx.blend_func(alpha, color), Err(Error::InvalidOperation));
        }
    }
    assert_eq!(ctx.blend_functions(), [ONE, ZERO, ONE, ZERO]);
    ctx.blend_func_separate(ONE, ZERO, CONSTANT_COLOR, CONSTANT_ALPHA)
        .unwrap();
    ctx.blend_func_separate(CONSTANT_COLOR, ONE, CONSTANT_ALPHA, ONE)
        .unwrap();
    let previous = ctx.blend_functions();
    for args in [
        [0xFFFF, ONE, ONE, ZERO],
        [ONE, SRC_ALPHA_SATURATE, ONE, ZERO],
        [ONE, ZERO, ONE, SRC_ALPHA_SATURATE],
        [ONE, ZERO, 0xFFFF, ONE],
    ] {
        assert_eq!(
            ctx.blend_func_separate(args[0], args[1], args[2], args[3]),
            Err(Error::InvalidEnum)
        );
        assert_eq!(ctx.blend_functions(), previous);
    }
    assert_eq!(
        ctx.blend_equation_separate(FUNC_SUBTRACT, 0x8007),
        Err(Error::InvalidEnum)
    );
    assert_eq!(ctx.blend_equations(), [FUNC_ADD; 2]);
    ctx.blend_func_separate(ONE, ZERO, SRC_ALPHA_SATURATE, ZERO)
        .unwrap();
}
#[test]
fn blend_obeys_depth_scissor_and_write_masks_but_clear_does_not_blend() {
    let (mut ctx, color, z) = scene();
    ctx.clear_color([0., 0., 1., 0.25]);
    ctx.clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT).unwrap();
    ctx.enable_blend(true);
    ctx.blend_func(ONE, ONE).unwrap();
    ctx.color_mask([true, false, false, true]);
    ctx.enable_depth_test(true);
    ctx.depth_mask(false);
    ctx.uniform_f32(z, &[-0.5]).unwrap();
    draw(&mut ctx, color, [0.2, 1., 0., 0.25]);
    ctx.uniform_f32(z, &[0.5]).unwrap();
    draw(&mut ctx, color, [0.2, 1., 0., 0.25]);
    near(pixel(&ctx), [0.4, 0., 1., 0.75]);
    ctx.depth_mask(true);
    ctx.uniform_f32(z, &[-0.5]).unwrap();
    draw(&mut ctx, color, [0.2, 0., 0., 0.25]);
    let front = pixel(&ctx);
    ctx.uniform_f32(z, &[0.5]).unwrap();
    draw(&mut ctx, color, [1., 1., 1., 1.]);
    assert_eq!(pixel(&ctx), front);
    ctx.scissor(0, 0, 1, 1).unwrap();
    ctx.enable_scissor(true);
    ctx.clear_color([0.1, 0.7, 0.3, 0.5]);
    ctx.clear(COLOR_BUFFER_BIT).unwrap();
    near(
        ctx.read_pixels(0, 0, 1, 1).unwrap().try_into().unwrap(),
        [0.1, 0., 1., 0.5],
    );
    assert_eq!(pixel(&ctx), front);
    ctx.resize(4, 4).unwrap();
    assert!(ctx.blend_enabled());
    assert_eq!(ctx.blend_functions(), [ONE; 4]);
}
#[test]
fn rgb_only_buffer_uses_destination_alpha_one_in_blending() {
    let (mut ctx, color, _) = scene();
    ctx.set_alpha_channel(false);
    ctx.clear_color([0.2, 0.4, 0.6, 0.]);
    ctx.clear(COLOR_BUFFER_BIT).unwrap();
    assert_eq!(pixel(&ctx), [51, 102, 153, 255]);
    ctx.enable_blend(true);
    ctx.blend_func(DST_ALPHA, ZERO).unwrap();
    draw(&mut ctx, color, [0.8, 0.4, 0.2, 0.1]);
    near(pixel(&ctx), [0.8, 0.4, 0.2, 1.]);
    ctx.blend_func(SRC_ALPHA_SATURATE, ONE).unwrap();
    draw(&mut ctx, color, [1., 1., 1., 0.5]);
    near(pixel(&ctx), [0.8, 0.4, 0.2, 1.]);
    ctx.resize(4, 4).unwrap();
    assert_eq!(pixel(&ctx), [0, 0, 0, 255]);
}
