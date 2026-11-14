use crate::tests::{run, setup};

#[test]
fn webgl_square_matrices_render_update_and_preserve_state_across_errors() {
    let (mut env, mut host) = setup("<canvas id=matrix1 width=8 height=4></canvas><canvas id=matrix2 width=8 height=4></canvas>");
    let page = include_str!("../../../tests/pages/webgl-matrices.html");
    let source = page.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    let out = run(&mut env, &mut host, source);
    assert_eq!(out, ["1 matrices ok", "2 matrices ok"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
