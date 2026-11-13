use rabbithole_webgl::{gl::*, Context, Error};
fn scene() -> Context {
    let mut c = Context::new_webgl2(4, 4).unwrap();
    let p = c.create_program();
    for (kind,source) in [(VERTEX_SHADER,"#version 300 es\nlayout(location=0) in vec2 position;layout(location=1) in vec4 color;out vec4 v;void main(){gl_Position=vec4(position,0.0,1.0);v=color;}"),(FRAGMENT_SHADER,"#version 300 es\nprecision mediump float;in vec4 v;out vec4 outputColor;void main(){outputColor=v;}")] {
  let s=c.create_shader(kind).unwrap(); c.shader_source(s,source).unwrap();c.compile_shader(s).unwrap();assert!(c.shader_compile_status(s).unwrap(),"{}",c.shader_info_log(s).unwrap());c.attach_shader(p,s).unwrap();
 }
    c.link_program(p).unwrap();
    assert!(c.program_link_status(p).unwrap());
    c.use_program(p).unwrap();
    c.vertex_attrib(1, [1., 0., 0., 1.]).unwrap();
    c
}
fn vertices(c: &mut Context) -> u32 {
    let b = c.create_buffer();
    c.bind_buffer(ARRAY_BUFFER, b).unwrap();
    c.buffer_data(
        ARRAY_BUFFER,
        &[-1f32, -1., 3., -1., -1., 3.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    c.vertex_attrib_pointer(0, 2, FLOAT, false, 0, 0).unwrap();
    c.enable_vertex_attrib_array(0, true).unwrap();
    b
}
fn elements(c: &mut Context, bytes: &[u8]) -> u32 {
    let b = c.create_buffer();
    c.bind_buffer(ELEMENT_ARRAY_BUFFER, b).unwrap();
    c.buffer_data(ELEMENT_ARRAY_BUFFER, bytes).unwrap();
    b
}
fn pixel(c: &Context) -> Vec<u8> {
    c.read_pixels(1, 1, 1, 1).unwrap()
}
#[test]
fn vao_switch_restores_pointers_and_elements_but_not_generic_constants_or_array_binding() {
    let mut c = scene();
    let a = c.create_vertex_array().unwrap();
    assert!(!c.is_vertex_array(a));
    c.bind_vertex_array(a).unwrap();
    assert!(c.is_vertex_array(a));
    let pos = vertices(&mut c);
    elements(&mut c, &[0, 1, 2]);
    let b = c.create_vertex_array().unwrap();
    c.bind_vertex_array(b).unwrap();
    assert_eq!(
        c.draw_elements(TRIANGLES, 3, UNSIGNED_BYTE, 0),
        Err(Error::InvalidOperation)
    );
    // ARRAY_BUFFER stayed global across the switch.
    c.vertex_attrib_pointer(0, 2, FLOAT, false, 0, 0).unwrap();
    c.enable_vertex_attrib_array(0, true).unwrap();
    elements(&mut c, &[0, 0, 0]);
    c.vertex_attrib(1, [0., 1., 0., 1.]).unwrap();
    c.bind_vertex_array(a).unwrap();
    c.draw_elements(TRIANGLES, 3, UNSIGNED_BYTE, 0).unwrap();
    assert_eq!(pixel(&c), [0, 255, 0, 255]);
    c.clear(COLOR_BUFFER_BIT).unwrap();
    c.bind_vertex_array(b).unwrap();
    c.draw_elements(TRIANGLES, 3, UNSIGNED_BYTE, 0).unwrap();
    assert_eq!(pixel(&c), [0; 4]);
    c.bind_vertex_array(a).unwrap();
    c.delete_vertex_array(a).unwrap();
    assert_eq!(c.vertex_array_binding(), 0);
    assert!(!c.is_vertex_array(a));
    assert_eq!(c.bind_vertex_array(a), Err(Error::InvalidOperation));
    c.bind_buffer(ARRAY_BUFFER, pos).unwrap();
    c.delete_vertex_array(b).unwrap();
}
#[test]
fn deleted_buffer_keeps_inactive_vao_storage_alive_until_vao_released() {
    let mut c = scene();
    let vao = c.create_vertex_array().unwrap();
    c.bind_vertex_array(vao).unwrap();
    let v = vertices(&mut c);
    let e = elements(&mut c, &[0, 1, 2]);
    c.bind_vertex_array(0).unwrap();
    c.delete_buffer(v).unwrap();
    c.delete_buffer(e).unwrap();
    assert_eq!(c.bind_buffer(ARRAY_BUFFER, v), Err(Error::InvalidOperation));
    c.bind_vertex_array(vao).unwrap();
    c.draw_elements(TRIANGLES, 3, UNSIGNED_BYTE, 0).unwrap();
    assert_eq!(pixel(&c), [255, 0, 0, 255]);
    c.delete_vertex_array(vao).unwrap();
    assert_eq!(
        c.draw_elements(TRIANGLES, 3, UNSIGNED_BYTE, 0),
        Err(Error::InvalidOperation)
    );
}
#[test]
fn uint32_indices_validate_alignment_bounds_and_primitive_restart_all_widths() {
    let mut c = scene();
    vertices(&mut c);
    for (kind, bytes) in [
        (UNSIGNED_BYTE, vec![0, 1, 2, 255, 0, 1, 2]),
        (
            UNSIGNED_SHORT,
            [0u16, 1, 2, 65535, 0, 1, 2]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
        (
            UNSIGNED_INT,
            [0u32, 1, 2, u32::MAX, 0, 1, 2]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect(),
        ),
    ] {
        elements(&mut c, &bytes);
        for mode in [TRIANGLES, TRIANGLE_STRIP, TRIANGLE_FAN] {
            c.clear(COLOR_BUFFER_BIT).unwrap();
            c.draw_elements(mode, 7, kind, 0).unwrap();
            assert_eq!(pixel(&c), [255, 0, 0, 255]);
        }
    }
    c.clear(COLOR_BUFFER_BIT).unwrap();
    assert_eq!(
        c.draw_elements(TRIANGLES, 3, UNSIGNED_INT, 2),
        Err(Error::InvalidOperation)
    );
    assert_eq!(
        c.draw_elements(TRIANGLES, 8, UNSIGNED_INT, 0),
        Err(Error::InvalidOperation)
    );
    elements(
        &mut c,
        &[0u32, 1, 2, 0, 1, 99999]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        c.draw_elements(TRIANGLES, 6, UNSIGNED_INT, 0),
        Err(Error::InvalidOperation)
    );
    assert_eq!(pixel(&c), [0; 4]);
}
#[test]
fn shader_ceiling_alone_does_not_enable_webgl2_objects_or_index_types() {
    for mut c in [
        Context::new(4, 4).unwrap(),
        Context::new_with_glsl_version(4, 4, 300).unwrap(),
    ] {
        assert_eq!(c.context_version(), 1);
        assert_eq!(c.create_vertex_array(), Err(Error::InvalidOperation));
        assert_eq!(c.bind_vertex_array(0), Err(Error::InvalidOperation));
        assert_eq!(
            c.draw_elements(TRIANGLES, 0, UNSIGNED_INT, 0),
            Err(Error::InvalidEnum)
        );
    }
}
