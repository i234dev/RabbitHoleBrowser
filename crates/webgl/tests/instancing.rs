use rabbithole_webgl::{gl::*, Context, Error};
fn setup(w: u32, h: u32) -> Context {
    let mut c = Context::new_webgl2(w, h).unwrap();
    let p = c.create_program();
    for (kind,src) in [(VERTEX_SHADER,"#version 300 es\nlayout(location=0) in vec2 position;layout(location=1) in vec2 offset;layout(location=2) in vec4 color;out vec4 v;void main(){gl_Position=vec4(position+offset,0.0,1.0);v=color;}"),(FRAGMENT_SHADER,"#version 300 es\nprecision mediump float;in vec4 v;out vec4 result;void main(){result=v;}")]{let s=c.create_shader(kind).unwrap();c.shader_source(s,src).unwrap();c.compile_shader(s).unwrap();assert!(c.shader_compile_status(s).unwrap(),"{}",c.shader_info_log(s).unwrap());c.attach_shader(p,s).unwrap();}
    c.link_program(p).unwrap();
    assert!(c.program_link_status(p).unwrap());
    c.use_program(p).unwrap();
    c
}
fn attr(c: &mut Context, index: u32, size: u32, data: &[f32], divisor: u32) -> u32 {
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    c.buffer_data(
        ARRAY_BUFFER,
        &data
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    c.vertex_attrib_pointer(index, size, FLOAT, false, 0, 0)
        .unwrap();
    c.enable_vertex_attrib_array(index, true).unwrap();
    c.vertex_attrib_divisor(index, divisor).unwrap();
    b
}
fn scene() -> Context {
    let mut c = setup(16, 8);
    attr(&mut c, 0, 2, &[-0.2, -0.8, 0.2, -0.8, 0., 0.8], 0);
    attr(&mut c, 1, 2, &[-0.5, 0., 0.5, 0.], 1);
    attr(&mut c, 2, 4, &[1., 0., 0., 1., 0., 1., 0., 1.], 1);
    c
}
fn pixel(c: &Context, x: i32) -> Vec<u8> {
    c.read_pixels(x, 4, 1, 1).unwrap()
}
#[test]
fn instanced_attributes_draw_different_positions_and_colors() {
    let mut c = scene();
    c.draw_arrays_instanced(TRIANGLES, 0, 3, 2).unwrap();
    assert_eq!(pixel(&c, 4), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 12), [0, 255, 0, 255]);
}
#[test]
fn divisors_advance_at_floor_instance_over_divisor_and_survive_vao_switch() {
    let mut c = scene();
    c.vertex_attrib_divisor(1, 2).unwrap();
    attr(
        &mut c,
        2,
        4,
        &[
            1., 0., 0., 1., 0., 1., 0., 1., 0., 0., 1., 1., 1., 1., 0., 1.,
        ],
        1,
    );
    c.draw_arrays_instanced(TRIANGLES, 0, 3, 4).unwrap();
    assert_eq!(pixel(&c, 4), [0, 255, 0, 255]);
    assert_eq!(pixel(&c, 12), [255, 255, 0, 255]);
    let vao = c.create_vertex_array().unwrap();
    c.bind_vertex_array(vao).unwrap();
    assert_eq!(c.vertex_attrib_divisor_value(1), Ok(0));
    c.vertex_attrib_divisor(1, 7).unwrap();
    c.bind_vertex_array(0).unwrap();
    assert_eq!(c.vertex_attrib_divisor_value(1), Ok(2));
    c.bind_vertex_array(vao).unwrap();
    assert_eq!(c.vertex_attrib_divisor_value(1), Ok(7));
}
#[test]
fn indexed_instancing_honors_uint32_restart_and_noninstanced_divisors() {
    let mut c = scene();
    let e = c.create_buffer();
    c.bind_buffer(ELEMENT_ARRAY_BUFFER, e).unwrap();
    c.buffer_data(
        ELEMENT_ARRAY_BUFFER,
        &[0u32, 1, 2, u32::MAX, 0, 1, 2]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    c.draw_elements_instanced(TRIANGLE_STRIP, 7, UNSIGNED_INT, 0, 2)
        .unwrap();
    assert_eq!(pixel(&c, 4), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 12), [0, 255, 0, 255]);
    c.clear(COLOR_BUFFER_BIT).unwrap();
    c.draw_arrays(TRIANGLES, 0, 3).unwrap();
    assert_eq!(pixel(&c, 4), [255, 0, 0, 255]);
    assert_eq!(pixel(&c, 12), [0; 4]);
}
#[test]
fn late_instance_ranges_and_overflow_are_checked_before_any_pixel_changes() {
    let mut c = scene();
    assert_eq!(
        c.draw_arrays_instanced(TRIANGLES, 0, 3, 3),
        Err(Error::InvalidOperation)
    );
    assert!(c.framebuffer_rgba().iter().all(|&b| b == 0));
    c.draw_arrays_instanced(TRIANGLES, 0, 3, 0).unwrap();
    assert_eq!(pixel(&c, 4), [0; 4]);
    assert_eq!(
        c.draw_arrays_instanced(TRIANGLES, u32::MAX, 3, 2),
        Err(Error::InvalidValue)
    );
    assert_eq!(c.vertex_attrib_divisor(16, 1), Err(Error::InvalidValue));
    let e = c.create_buffer();
    c.bind_buffer(ELEMENT_ARRAY_BUFFER, e).unwrap();
    c.buffer_data(
        ELEMENT_ARRAY_BUFFER,
        &[0u32, 1, 2]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(
        c.draw_elements_instanced(TRIANGLES, 3, UNSIGNED_INT, 2, 2),
        Err(Error::InvalidOperation)
    );
    assert!(c.framebuffer_rgba().iter().all(|&b| b == 0));
}
#[test]
fn instance_work_budget_rejects_the_entire_batch_atomically() {
    let mut c = setup(128, 128);
    attr(&mut c, 0, 2, &[-1., -1., 3., -1., -1., 3.], 0);
    attr(&mut c, 1, 2, &[0., 0.], u32::MAX);
    attr(&mut c, 2, 4, &[1., 0., 0., 1.], u32::MAX);
    assert_eq!(
        c.draw_arrays_instanced(TRIANGLES, 0, 3, 1024),
        Err(Error::OutOfMemory)
    );
    assert!(c.framebuffer_rgba().iter().all(|&b| b == 0));
    assert_eq!(
        c.draw_arrays_instanced(TRIANGLES, 0, 3, 1025),
        Err(Error::OutOfMemory)
    );
    assert!(c.framebuffer_rgba().iter().all(|&b| b == 0));
}
#[test]
fn instancing_is_webgl2_only_and_integer_instance_builtin_is_rejected() {
    let mut c = Context::new(1, 1).unwrap();
    assert_eq!(c.vertex_attrib_divisor(0, 1), Err(Error::InvalidOperation));
    assert_eq!(
        c.draw_arrays_instanced(TRIANGLES, 0, 3, 1),
        Err(Error::InvalidOperation)
    );
    assert_eq!(
        c.draw_elements_instanced(TRIANGLES, 3, UNSIGNED_SHORT, 0, 1),
        Err(Error::InvalidOperation)
    );
    let mut c = Context::new_webgl2(1, 1).unwrap();
    let s = c.create_shader(VERTEX_SHADER).unwrap();
    c.shader_source(
        s,
        "#version 300 es\nvoid main(){gl_Position=vec4(float(gl_InstanceID));}",
    )
    .unwrap();
    c.compile_shader(s).unwrap();
    assert!(!c.shader_compile_status(s).unwrap());
    assert!(!c.shader_info_log(s).unwrap().is_empty());
}
