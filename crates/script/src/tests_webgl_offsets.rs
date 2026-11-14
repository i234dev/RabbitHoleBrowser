//! Conversion is checked through observable GL state and the actual rasterizer.
use crate::tests::{run, setup};

#[test]
fn webgl_signed_64_sizes_convert_before_memory_budget_validation() {
    let (mut env, mut host) = setup("<canvas id=a></canvas><canvas id=b></canvas>");
    let out = run(&mut env, &mut host, r#"
      for (const [id, version] of [['a',1],['b',2]]) {
        const gl=document.getElementById(id).getContext(version===2?'webgl2':'webgl');
        const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);
        for (const [input,size] of [[NaN,0],[Infinity,0],[-Infinity,0],[-0,0],[-0.9,0],[1.9,1],[7.9,7],[2**64,0],[-(2**64),0],[2**64+4096,4096],[-(2**64)+4096,4096]]) {
          gl.bufferData(gl.ARRAY_BUFFER,input,gl.STATIC_DRAW);
          console.log(gl.getError()===gl.NO_ERROR,gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE)===size);
        }
        gl.bufferData(gl.ARRAY_BUFFER,8,gl.STATIC_DRAW);
        for (const input of [-1,-1.9,2**63,-(2**63),-(2**64)-4096]) {
          gl.bufferData(gl.ARRAY_BUFFER,input,gl.STATIC_DRAW);
          console.log(gl.getError()===gl.INVALID_VALUE,gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE)===8);
        }
        for (const input of [64*1024*1024+1.9,2**32,2**53,2**63-1024,-(2**63)-2048]) {
          gl.bufferData(gl.ARRAY_BUFFER,input,gl.STATIC_DRAW);
          console.log(gl.getError()===gl.OUT_OF_MEMORY,gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE)===8);
        }
      }
    "#);
    assert_eq!(out, vec!["true true"; 42]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}

#[test]
fn webgl_signed_64_offsets_reach_real_webgl1_and_webgl2_pixels() {
    let html = include_str!("../../../tests/pages/webgl-idl-offsets.html");
    let (mut env, mut host) = setup(html);
    let source = html.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    let out = run(&mut env, &mut host, source);
    assert_eq!(out, ["1 0,255,0,255", "2 0,255,0,255"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
