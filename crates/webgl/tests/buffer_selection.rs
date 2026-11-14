use rabbithole_webgl::{gl::*, Context, Error};

fn fbo(c: &mut Context, depth: bool) -> u32 {
    let t = c.create_texture();
    c.bind_texture(TEXTURE_2D, t).unwrap();
    c.tex_image_2d(TEXTURE_2D, 0, RGBA, 1, 1, 0, RGBA, UNSIGNED_BYTE, None).unwrap();
    let f = c.create_framebuffer();
    c.bind_framebuffer(FRAMEBUFFER, f).unwrap();
    c.framebuffer_texture_2d(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, t, 0).unwrap();
    if depth {
        let r = c.create_renderbuffer();
        c.bind_renderbuffer(RENDERBUFFER, r).unwrap();
        c.renderbuffer_storage(RENDERBUFFER, DEPTH_COMPONENT16, 1, 1).unwrap();
        c.framebuffer_renderbuffer(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, r).unwrap();
    }
    f
}
fn pixel(c: &mut Context, f: u32) -> Vec<u8> {
    c.bind_framebuffer(READ_FRAMEBUFFER, f).unwrap();
    c.read_buffer_select(if f == 0 { BACK } else { COLOR_ATTACHMENT0 }).unwrap();
    c.read_pixels(0, 0, 1, 1).unwrap()
}
fn draw(c: &mut Context, z: f32, color: [f32;4]) {
    let p = c.create_program();
    for (kind, source) in [
        (VERTEX_SHADER, "attribute vec2 p;uniform float z;void main(){gl_Position=vec4(p,z,1.0);}"),
        (FRAGMENT_SHADER, "precision mediump float;uniform vec4 color;void main(){gl_FragColor=color;}"),
    ] {
        let s = c.create_shader(kind).unwrap();
        c.shader_source(s, source).unwrap(); c.compile_shader(s).unwrap();
        assert!(c.shader_compile_status(s).unwrap(), "{}", c.shader_info_log(s).unwrap());
        c.attach_shader(p, s).unwrap();
    }
    c.link_program(p).unwrap(); assert!(c.program_link_status(p).unwrap(), "{}", c.program_info_log(p).unwrap());
    c.use_program(p).unwrap();
    c.uniform_f32(c.get_uniform_location(p, "z").unwrap().unwrap(), &[z]).unwrap();
    c.uniform_f32(c.get_uniform_location(p, "color").unwrap().unwrap(), &color).unwrap();
    let b = c.create_buffer(); c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    c.buffer_data(ARRAY_BUFFER, &[-1f32,-1.,3.,-1.,-1.,3.].into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>()).unwrap();
    let a = c.get_attrib_location(p, "p").unwrap() as u32;
    c.vertex_attrib_pointer(a, 2, FLOAT, false, 0, 0).unwrap(); c.enable_vertex_attrib_array(a, true).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
}

#[test]
fn read_and_draw_selections_belong_to_each_bound_framebuffer() {
    let mut c = Context::new_webgl2(1, 1).unwrap();
    assert_eq!(c.read_buffer_selection().unwrap(), BACK); assert_eq!(c.draw_buffer_selection().unwrap(), BACK);
    c.read_buffer_select(NONE).unwrap(); c.draw_buffers(&[NONE]).unwrap();
    let a = fbo(&mut c, false); assert_eq!(c.read_buffer_selection().unwrap(), COLOR_ATTACHMENT0);
    assert_eq!(c.draw_buffer_selection().unwrap(), COLOR_ATTACHMENT0);
    c.read_buffer_select(NONE).unwrap(); c.draw_buffers(&[]).unwrap();
    let b = fbo(&mut c, false);
    c.bind_framebuffer(READ_FRAMEBUFFER, a).unwrap(); assert_eq!(c.read_buffer_selection().unwrap(), NONE);
    assert_eq!(c.draw_buffer_selection().unwrap(), COLOR_ATTACHMENT0);
    c.bind_framebuffer(DRAW_FRAMEBUFFER, a).unwrap(); assert_eq!(c.draw_buffer_selection().unwrap(), NONE);
    c.bind_framebuffer(READ_FRAMEBUFFER, b).unwrap(); assert_eq!(c.read_buffer_selection().unwrap(), COLOR_ATTACHMENT0);
    c.bind_framebuffer(FRAMEBUFFER, 0).unwrap(); assert_eq!(c.read_buffer_selection().unwrap(), NONE);
    assert_eq!(c.draw_buffer_selection().unwrap(), NONE);
    c.resize(2, 2).unwrap(); assert_eq!(c.read_buffer_selection().unwrap(), NONE); assert_eq!(c.draw_buffer_selection().unwrap(), NONE);
}

#[test]
fn none_suppresses_color_clears_and_draws_while_depth_still_changes() {
    let mut c = Context::new_webgl2(1, 1).unwrap(); let f = fbo(&mut c, true);
    c.clear_color([1.,0.,0.,1.]); c.clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT).unwrap();
    c.draw_buffers(&[NONE]).unwrap(); c.clear_color([0.,1.,0.,1.]); c.clear_depth(0.75);
    c.clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT).unwrap(); assert_eq!(pixel(&mut c, f), [255,0,0,255]);
    c.enable_depth_test(true); draw(&mut c, -0.5, [0.,1.,0.,1.]);
    assert_eq!(pixel(&mut c, f), [255,0,0,255]);
    c.draw_buffers(&[COLOR_ATTACHMENT0]).unwrap(); draw(&mut c, 0.5, [0.,0.,1.,1.]);
    assert_eq!(pixel(&mut c, f), [255,0,0,255], "depth written with disabled color must reject the farther draw");
    draw(&mut c, -0.75, [0.,0.,1.,1.]); assert_eq!(pixel(&mut c, f), [0,0,255,255]);
    c.bind_framebuffer(FRAMEBUFFER, 0).unwrap(); c.draw_buffers(&[NONE]).unwrap(); c.clear_color([1.,0.,0.,1.]); c.clear(COLOR_BUFFER_BIT).unwrap();
    assert_eq!(pixel(&mut c, 0), [0,0,0,0]);
    c.draw_buffers(&[BACK]).unwrap(); c.clear(COLOR_BUFFER_BIT).unwrap(); assert_eq!(pixel(&mut c, 0), [255,0,0,255]);
}

#[test]
fn selection_errors_are_atomic_and_webgl1_does_not_accept_the_commands() {
    let mut c = Context::new_webgl2(1, 1).unwrap();
    for (buffers, error) in [(&[][..], Error::InvalidOperation), (&[COLOR_ATTACHMENT0][..], Error::InvalidOperation),
        (&[BACK,BACK][..], Error::InvalidValue), (&[123][..], Error::InvalidEnum)] {
        assert_eq!(c.draw_buffers(buffers), Err(error)); assert_eq!(c.draw_buffer_selection().unwrap(), BACK);
    }
    assert_eq!(c.read_buffer_select(COLOR_ATTACHMENT0), Err(Error::InvalidOperation));
    assert_eq!(c.read_buffer_select(123), Err(Error::InvalidEnum)); assert_eq!(c.read_buffer_selection().unwrap(), BACK);
    fbo(&mut c, false);
    for selection in [BACK, COLOR_ATTACHMENT0 + 1] {
        assert_eq!(c.read_buffer_select(selection), Err(Error::InvalidOperation));
        assert_eq!(c.draw_buffers(&[selection]), Err(Error::InvalidOperation));
    }
    assert_eq!(c.read_buffer_selection().unwrap(), COLOR_ATTACHMENT0); assert_eq!(c.draw_buffer_selection().unwrap(), COLOR_ATTACHMENT0);
    c.read_buffer_select(NONE).unwrap(); assert_eq!(c.read_pixels(0,0,1,1), Err(Error::InvalidOperation));
    let mut old = Context::new(1, 1).unwrap(); assert_eq!(old.read_buffer_select(NONE), Err(Error::InvalidOperation));
    assert_eq!(old.draw_buffers(&[NONE]), Err(Error::InvalidOperation));
}

#[test]
fn blits_obey_none_without_losing_depth_copies_or_error_atomicity() {
    let mut c = Context::new_webgl2(1, 1).unwrap(); let src = fbo(&mut c, true);
    c.clear_color([1.,0.,0.,1.]); c.clear_depth(0.25); c.clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT).unwrap();
    c.read_buffer_select(NONE).unwrap(); let dst = fbo(&mut c, true);
    c.bind_framebuffer(READ_FRAMEBUFFER, src).unwrap();
    assert_eq!(c.blit_framebuffer([0,0,1,1],[0,0,1,1],COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT,NEAREST), Err(Error::InvalidOperation));
    assert_eq!(pixel(&mut c, dst), [0,0,0,0]);
    c.bind_framebuffer(READ_FRAMEBUFFER, src).unwrap(); c.draw_buffers(&[NONE]).unwrap();
    c.blit_framebuffer([0,0,1,1],[0,0,1,1],COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT,NEAREST).unwrap();
    c.draw_buffers(&[COLOR_ATTACHMENT0]).unwrap(); c.enable_depth_test(true); draw(&mut c, 0., [0.,1.,0.,1.]);
    assert_eq!(pixel(&mut c, dst), [0,0,0,0], "depth copied even though read and draw color are NONE");
    c.bind_framebuffer(READ_FRAMEBUFFER, src).unwrap(); c.read_buffer_select(COLOR_ATTACHMENT0).unwrap();
    c.blit_framebuffer([0,0,1,1],[0,0,1,1],COLOR_BUFFER_BIT,NEAREST).unwrap(); assert_eq!(pixel(&mut c, dst), [255,0,0,255]);
}

#[test]
fn deleting_bound_fbos_restores_the_existing_default_selections() {
    let mut c = Context::new_webgl2(1, 1).unwrap(); c.read_buffer_select(NONE).unwrap(); c.draw_buffers(&[NONE]).unwrap();
    let f = fbo(&mut c, false); c.delete_framebuffer(f).unwrap();
    assert_eq!(c.framebuffer_binding(READ_FRAMEBUFFER).unwrap(), 0); assert_eq!(c.framebuffer_binding(DRAW_FRAMEBUFFER).unwrap(), 0);
    assert_eq!(c.read_buffer_selection().unwrap(), NONE); assert_eq!(c.draw_buffer_selection().unwrap(), NONE);
    c.draw_buffers(&[BACK]).unwrap(); let next = fbo(&mut c, false);
    assert_ne!(next, f); assert_eq!(c.read_buffer_selection().unwrap(), COLOR_ATTACHMENT0); assert_eq!(c.draw_buffer_selection().unwrap(), COLOR_ATTACHMENT0);
}
