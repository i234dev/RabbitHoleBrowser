use crate::tests::{run, setup};

#[test]
fn webgl_front_face_state_and_fragment_bool_control_real_pixels() {
    let (mut env, mut host) = setup("<canvas id=gl1 width=4 height=4></canvas><canvas id=gl2 width=4 height=4></canvas>");
    let page = include_str!("../../../tests/pages/webgl-facing.html");
    let source = page.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    assert_eq!(run(&mut env, &mut host, source), ["1 facing ok", "2 facing ok"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
