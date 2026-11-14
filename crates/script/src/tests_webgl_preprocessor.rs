use crate::tests::{run, setup};

#[test]
fn webgl_preprocessed_shader_pixels_and_compile_errors_cross_the_js_binding() {
    let html = include_str!("../../../tests/pages/webgl-preprocessor.html");
    let (mut env, mut host) = setup(html);
    let source = html.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    let out = run(&mut env, &mut host, source);
    assert_eq!(out, ["1 0,255,0,255", "2 0,255,0,255"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
