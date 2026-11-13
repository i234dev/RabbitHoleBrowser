use rabbithole_webgl::{gl::*, Context, Error};

fn texture(c: &mut Context, w: u32, h: u32) -> u32 {
    let t = c.create_texture();
    c.bind_texture(TEXTURE_2D, t).unwrap();
    c.tex_image_2d(TEXTURE_2D, 0, RGBA, w, h, 0, RGBA, UNSIGNED_BYTE, None)
        .unwrap();
    for pname in [TEXTURE_MIN_FILTER, TEXTURE_MAG_FILTER] {
        c.tex_parameter_i(TEXTURE_2D, pname, NEAREST as i32)
            .unwrap();
    }
    t
}
fn framebuffer(c: &mut Context, texture: u32) -> u32 {
    let f = c.create_framebuffer();
    c.bind_framebuffer(FRAMEBUFFER, f).unwrap();
    c.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, texture, 0)
        .unwrap();
    f
}
fn pixel(c: &Context, x: i32, y: i32) -> [u8; 4] {
    c.read_pixels(x, y, 1, 1).unwrap().try_into().unwrap()
}
fn clear(c: &mut Context, color: [f32; 4]) {
    c.clear_color(color);
    c.clear(COLOR_BUFFER_BIT).unwrap();
}
fn program(c: &mut Context, fragment: &str) -> u32 {
    let p = c.create_program();
    for (kind, source) in [
        (VERTEX_SHADER, "attribute vec2 position;uniform float z;varying vec2 uv;void main(){gl_Position=vec4(position,z,1.0);uv=position*0.5+vec2(0.5);}"),
        (FRAGMENT_SHADER, fragment),
    ] {
        let s = c.create_shader(kind).unwrap(); c.shader_source(s, source).unwrap(); c.compile_shader(s).unwrap();
        assert!(c.shader_compile_status(s).unwrap(), "{}", c.shader_info_log(s).unwrap()); c.attach_shader(p, s).unwrap();
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
    c.buffer_data(
        ARRAY_BUFFER,
        &vertices
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let a = c.get_attrib_location(p, "position").unwrap() as u32;
    c.vertex_attrib_pointer(a, 2, FLOAT, false, 0, 0).unwrap();
    c.enable_vertex_attrib_array(a, true).unwrap();
    p
}

#[test]
fn framebuffer_clear_is_separate_from_default_and_readback_uses_attachment_size() {
    for mut c in [
        Context::new(2, 3).unwrap(),
        Context::new_webgl2(2, 3).unwrap(),
    ] {
        c.set_alpha_channel(false);
        clear(&mut c, [0., 0., 1., 1.]);
        let t = texture(&mut c, 3, 2);
        let f = framebuffer(&mut c, t);
        assert_eq!(
            c.check_framebuffer_status(FRAMEBUFFER),
            Ok(FRAMEBUFFER_COMPLETE)
        );
        assert_eq!(c.dimensions(), (2, 3));
        assert_eq!(c.read_dimensions(), Ok((3, 2)));
        clear(&mut c, [1., 0., 0., 0.5]);
        c.scissor(0, 0, 1, 1).unwrap();
        c.enable_scissor(true);
        clear(&mut c, [0., 1., 0., 0.25]);
        c.enable_scissor(false);
        assert_eq!(pixel(&c, 0, 0), [0, 255, 0, 64]);
        assert_eq!(pixel(&c, 2, 1), [255, 0, 0, 128]);
        assert!(c
            .framebuffer_rgba()
            .chunks_exact(4)
            .all(|p| p == [0, 0, 255, 255]));
        assert_eq!(c.viewport_rect(), [0, 0, 2, 3]);
        c.resize(5, 1).unwrap();
        assert_eq!(c.read_dimensions(), Ok((3, 2)));
        assert_eq!(pixel(&c, 0, 0), [0, 255, 0, 64]);
        c.bind_framebuffer(FRAMEBUFFER, 0).unwrap();
        assert_eq!(c.read_dimensions(), Ok((5, 1)));
        assert!(c
            .framebuffer_rgba()
            .chunks_exact(4)
            .all(|p| p == [0, 0, 0, 255]));
        c.bind_framebuffer(FRAMEBUFFER, f).unwrap();
        assert_eq!(pixel(&c, 2, 1), [255, 0, 0, 128]);
    }
}

#[test]
fn rendering_to_texture_then_sampling_preserves_lower_left_orientation() {
    let mut c = Context::new(2, 2).unwrap();
    let t = texture(&mut c, 2, 2);
    framebuffer(&mut c, t);
    program(&mut c, "precision mediump float;void main(){gl_FragColor=vec4(gl_FragCoord.x/2.0,gl_FragCoord.y/2.0,0.0,1.0);}");
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 0), [64, 64, 0, 255]);
    assert_eq!(pixel(&c, 1, 1), [191, 191, 0, 255]);
    let offscreen = c.read_pixels(0, 0, 2, 2).unwrap();
    c.bind_framebuffer(FRAMEBUFFER, 0).unwrap();
    program(&mut c, "precision mediump float;uniform sampler2D image;varying vec2 uv;void main(){gl_FragColor=texture2D(image,uv);}");
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(c.read_pixels(0, 0, 2, 2).unwrap(), offscreen);
    assert_eq!(&c.framebuffer_rgba()[..4], &[64, 191, 0, 255]);
}

#[test]
fn framebuffer_feedback_fails_before_modifying_texture_or_default() {
    let mut c = Context::new(2, 2).unwrap();
    clear(&mut c, [0., 0., 1., 1.]);
    let t = texture(&mut c, 2, 2);
    framebuffer(&mut c, t);
    clear(&mut c, [1., 0., 0., 1.]);
    program(&mut c, "precision mediump float;uniform sampler2D image;varying vec2 uv;void main(){gl_FragColor=texture2D(image,uv);}");
    assert_eq!(c.draw_arrays(TRIANGLES, 0, 3), Err(Error::InvalidOperation));
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
    assert_eq!(c.dimensions(), (2, 2));
    assert!(c
        .framebuffer_rgba()
        .chunks_exact(4)
        .all(|p| p == [0, 0, 255, 255]));
    c.bind_framebuffer(FRAMEBUFFER, 0).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 1, 1), [255, 0, 0, 255]);
}

#[test]
fn depth_renderbuffer_controls_depth_test_and_clear_masks() {
    let mut c = Context::new(2, 2).unwrap();
    c.set_depth_channel(false);
    c.enable_depth_test(true);
    let t = texture(&mut c, 2, 2);
    framebuffer(&mut c, t);
    let r = c.create_renderbuffer();
    c.bind_renderbuffer(RENDERBUFFER, r).unwrap();
    c.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 2, 2)
        .unwrap();
    c.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, r)
        .unwrap();
    assert_eq!(
        c.renderbuffer_parameter(RENDERBUFFER, RENDERBUFFER_DEPTH_SIZE),
        Ok(16)
    );
    assert_eq!(
        c.check_framebuffer_status(FRAMEBUFFER),
        Ok(FRAMEBUFFER_COMPLETE)
    );
    let p = program(
        &mut c,
        "precision mediump float;uniform vec4 color;void main(){gl_FragColor=color;}",
    );
    let color = c.get_uniform_location(p, "color").unwrap().unwrap();
    let z = c.get_uniform_location(p, "z").unwrap().unwrap();
    c.enable_depth_test(true);
    for (depth, value) in [
        (0.8, [1., 0., 0., 1.]),
        (-0.8, [0., 1., 0., 1.]),
        (0.8, [0., 0., 1., 1.]),
    ] {
        c.uniform_f32(z, &[depth]).unwrap();
        c.uniform_f32(color, &value).unwrap();
        c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    }
    assert_eq!(pixel(&c, 0, 0), [0, 255, 0, 255]);
    c.depth_mask(false);
    c.clear(DEPTH_BUFFER_BIT).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 0), [0, 255, 0, 255]);
    c.depth_mask(true);
    c.clear(DEPTH_BUFFER_BIT).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 0), [0, 0, 255, 255]);
    c.delete_renderbuffer(r).unwrap();
    assert_eq!(
        c.check_framebuffer_status(FRAMEBUFFER),
        Ok(FRAMEBUFFER_COMPLETE)
    );
    c.uniform_f32(z, &[0.9]).unwrap();
    c.uniform_f32(color, &[1., 0., 0., 1.]).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
    c.bind_framebuffer(FRAMEBUFFER, 0).unwrap();
    for (depth, value) in [(-0.8, [0., 1., 0., 1.]), (0.8, [1., 0., 0., 1.])] {
        c.uniform_f32(z, &[depth]).unwrap();
        c.uniform_f32(color, &value).unwrap();
        c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    }
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
}

#[test]
fn attachment_completeness_and_errors_do_not_damage_default_state() {
    for mut c in [
        Context::new(2, 2).unwrap(),
        Context::new_webgl2(2, 2).unwrap(),
    ] {
        clear(&mut c, [0., 1., 0., 1.]);
        let f = c.create_framebuffer();
        c.bind_framebuffer(FRAMEBUFFER, f).unwrap();
        assert_eq!(
            c.check_framebuffer_status(FRAMEBUFFER),
            Ok(FRAMEBUFFER_INCOMPLETE_MISSING_ATTACHMENT)
        );
        assert_eq!(
            c.clear(COLOR_BUFFER_BIT),
            Err(Error::InvalidFramebufferOperation)
        );
        assert_eq!(
            c.read_pixels(0, 0, 1, 1),
            Err(Error::InvalidFramebufferOperation)
        );
        let t = c.create_texture();
        c.bind_texture(TEXTURE_2D, t).unwrap();
        c.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, t, 0)
            .unwrap();
        assert_eq!(
            c.check_framebuffer_status(FRAMEBUFFER),
            Ok(FRAMEBUFFER_INCOMPLETE_ATTACHMENT)
        );
        c.bind_texture(TEXTURE_2D, t).unwrap();
        c.tex_image_2d(TEXTURE_2D, 0, RGBA, 2, 2, 0, RGBA, UNSIGNED_BYTE, None)
            .unwrap();
        let r = c.create_renderbuffer();
        c.bind_renderbuffer(RENDERBUFFER, r).unwrap();
        c.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 1, 1)
            .unwrap();
        c.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, r)
            .unwrap();
        let status = c.check_framebuffer_status(FRAMEBUFFER).unwrap();
        assert!(status == FRAMEBUFFER_INCOMPLETE_DIMENSIONS || status == FRAMEBUFFER_UNSUPPORTED);
        assert_eq!(
            c.clear(COLOR_BUFFER_BIT),
            Err(Error::InvalidFramebufferOperation)
        );
        c.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 2, 2)
            .unwrap();
        assert_eq!(
            c.check_framebuffer_status(FRAMEBUFFER),
            Ok(FRAMEBUFFER_COMPLETE)
        );
        assert_eq!(
            c.framebuffer_attachment_parameter(
                FRAMEBUFFER,
                COLOR_ATTACHMENT0,
                FRAMEBUFFER_ATTACHMENT_OBJECT_TYPE
            ),
            Ok(TEXTURE)
        );
        assert_eq!(
            c.framebuffer_attachment_parameter(
                FRAMEBUFFER,
                DEPTH_ATTACHMENT,
                FRAMEBUFFER_ATTACHMENT_OBJECT_NAME
            ),
            Ok(r)
        );
        c.delete_framebuffer(f).unwrap();
        assert_eq!(pixel(&c, 0, 0), [0, 255, 0, 255]);
        assert_eq!(
            c.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, t, 0),
            Err(Error::InvalidOperation)
        );
        assert_eq!(
            c.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 4097, 1),
            Err(Error::InvalidValue)
        );
    }
}

#[test]
fn deleted_resources_remain_in_inactive_framebuffers_until_detached() {
    let mut c = Context::new(2, 2).unwrap();
    let t = texture(&mut c, 2, 2);
    let f = framebuffer(&mut c, t);
    let r = c.create_renderbuffer();
    c.bind_renderbuffer(RENDERBUFFER, r).unwrap();
    c.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 2, 2)
        .unwrap();
    c.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, r)
        .unwrap();
    clear(&mut c, [1., 0., 0., 1.]);
    c.bind_framebuffer(FRAMEBUFFER, 0).unwrap();
    c.delete_texture(t).unwrap();
    c.delete_renderbuffer(r).unwrap();
    assert_eq!(c.bind_texture(TEXTURE_2D, t), Err(Error::InvalidOperation));
    assert_eq!(
        c.bind_renderbuffer(RENDERBUFFER, r),
        Err(Error::InvalidOperation)
    );
    c.bind_framebuffer(FRAMEBUFFER, f).unwrap();
    assert_eq!(
        c.check_framebuffer_status(FRAMEBUFFER),
        Ok(FRAMEBUFFER_COMPLETE)
    );
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
    c.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, 0, 0)
        .unwrap();
    assert_eq!(c.texture_dimensions(t), Err(Error::InvalidValue));
    c.delete_framebuffer(f).unwrap();
    assert_eq!(
        c.renderbuffer_parameter(RENDERBUFFER, RENDERBUFFER_WIDTH),
        Err(Error::InvalidOperation)
    );
}

#[test]
fn webgl2_read_and_draw_framebuffer_bindings_are_independent() {
    let mut c = Context::new_webgl2(2, 2).unwrap();
    let a = texture(&mut c, 1, 1);
    let fa = framebuffer(&mut c, a);
    clear(&mut c, [1., 0., 0., 1.]);
    let b = texture(&mut c, 3, 2);
    let fb = framebuffer(&mut c, b);
    clear(&mut c, [0., 1., 0., 1.]);
    c.bind_framebuffer(READ_FRAMEBUFFER, fa).unwrap();
    assert_eq!(c.read_dimensions(), Ok((1, 1)));
    clear(&mut c, [0., 0., 1., 1.]);
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
    assert_eq!(c.framebuffer_binding(DRAW_FRAMEBUFFER), Ok(fb));
    c.bind_framebuffer(READ_FRAMEBUFFER, fb).unwrap();
    assert_eq!(pixel(&c, 2, 1), [0, 0, 255, 255]);
    c.delete_framebuffer(fb).unwrap();
    assert_eq!(c.framebuffer_binding(READ_FRAMEBUFFER), Ok(0));
    assert_eq!(c.framebuffer_binding(DRAW_FRAMEBUFFER), Ok(0));
    let mut gl1 = Context::new(1, 1).unwrap();
    assert_eq!(
        gl1.bind_framebuffer(READ_FRAMEBUFFER, 0),
        Err(Error::InvalidEnum)
    );
    assert_eq!(
        gl1.check_framebuffer_status(DRAW_FRAMEBUFFER),
        Err(Error::InvalidEnum)
    );
}

#[test]
fn depth_component16_quantizes_before_depth_comparison() {
    let mut c = Context::new(1, 1).unwrap();
    let t = texture(&mut c, 1, 1);
    framebuffer(&mut c, t);
    let rb = c.create_renderbuffer();
    c.bind_renderbuffer(RENDERBUFFER, rb).unwrap();
    c.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 1, 1)
        .unwrap();
    c.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, rb)
        .unwrap();
    let p = program(
        &mut c,
        "precision mediump float;uniform vec4 color;void main(){gl_FragColor=color;}",
    );
    let color = c.get_uniform_location(p, "color").unwrap().unwrap();
    let z = c.get_uniform_location(p, "z").unwrap().unwrap();
    c.enable_depth_test(true);
    c.uniform_f32(z, &[0.]).unwrap();
    c.uniform_f32(color, &[0., 1., 0., 1.]).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    c.depth_func(EQUAL).unwrap();
    c.uniform_f32(z, &[0.000001]).unwrap();
    c.uniform_f32(color, &[1., 0., 0., 1.]).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 0, 0), [255, 0, 0, 255]);
}
