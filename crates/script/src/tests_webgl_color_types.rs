use crate::tests::{run, setup};

#[test]
fn webgl_vector_masks_and_constant_parameters_reach_real_pixels() {
    let (mut env, mut host) = setup("<canvas id=gl1 width=4 height=4></canvas><canvas id=gl2 width=4 height=4></canvas>");
    let page = include_str!("../../../tests/pages/webgl-color-types.html");
    let source = page.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    assert_eq!(run(&mut env, &mut host, source), ["1 color types ok", "2 color types ok"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
