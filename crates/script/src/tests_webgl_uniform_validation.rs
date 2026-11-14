use crate::tests::{run, setup};

#[test]
fn webgl_uniform_sequences_convert_before_gl_errors_and_preserve_real_pixels() {
    let (mut env, mut host) = setup("<canvas id=uniform1 width=1 height=1></canvas><canvas id=uniform2 width=1 height=1></canvas>");
    let page = include_str!("../../../tests/pages/webgl-uniform-validation.html");
    let source = page.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    let output = run(&mut env, &mut host, source);
    assert_eq!(output, ["1 uniform validation ok", "2 uniform validation ok"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
