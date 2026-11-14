use crate::tests::{run, setup};

#[test]
fn webgl_precision_headers_and_real_validation_status_reach_pixels() {
    let (mut env, mut host) = setup("<canvas id=gl1 width=4 height=4></canvas><canvas id=gl2 width=4 height=4></canvas>");
    let page = include_str!("../../../tests/pages/webgl-precision-validation.html");
    let source = page.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    assert_eq!(run(&mut env, &mut host, source), ["1 precision validation ok", "2 precision validation ok"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
