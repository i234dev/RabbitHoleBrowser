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
        for (const input of [-1,-1.9,-1024,2**63,-(2**63),-(2**64)-4096]) {
          gl.bufferData(gl.ARRAY_BUFFER,input,gl.STATIC_DRAW);
          console.log(gl.getError()===gl.INVALID_VALUE,gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE)===8);
        }
        for (const input of [64*1024*1024+1.9,2**32,2**53,2**63-1024,-(2**63)-2048]) {
          gl.bufferData(gl.ARRAY_BUFFER,input,gl.STATIC_DRAW);
          console.log(gl.getError()===gl.OUT_OF_MEMORY,gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE)===8);
        }
      }
    "#);
    assert_eq!(out, vec!["true true"; 44]);
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

#[test]
fn webgl_signed_64_conversion_keeps_bootstrap_intrinsics_after_public_replacement() {
    let html = include_str!("../../../tests/pages/webgl-idl-offsets.html");
    let pixels = html.split_once("<script>").unwrap().1.split_once("</script>").unwrap().0;
    for (mode, truncate, finite) in [
        ("wrong results", "return 0;", "return false;"),
        ("throwing", "throw sentinel;", "throw sentinel;"),
    ] {
        let (mut env, mut host) = setup(html);
        let source = r#"
          const savedTrunc=Math.trunc,savedFinite=Number.isFinite;
          const sentinel=new Error('public offset intrinsics must not be called');
          let publicCalls=0;
          try {
            Math.trunc=()=>{publicCalls++;TRUNCATE_RESULT};
            Number.isFinite=()=>{publicCalls++;FINITE_RESULT};
            // The API was installed by ScriptHost before page scripts, but
            // these contexts are created only after the public replacements.
            for(const [id,type] of [['gl1','webgl'],['gl2','webgl2']]) {
              const gl=document.getElementById(id).getContext(type,{preserveDrawingBuffer:true,premultipliedAlpha:false});
              gl.bindBuffer(gl.ARRAY_BUFFER,gl.createBuffer());
              gl.bufferData(gl.ARRAY_BUFFER,8,gl.STATIC_DRAW);
              const size=gl.getBufferParameter(gl.ARRAY_BUFFER,gl.BUFFER_SIZE);
              if(gl.getError()!==gl.NO_ERROR||size!==8)throw new Error('numeric allocation changed');
              console.log('size',id,size);
            }
            PIXEL_FIXTURE
            if(publicCalls!==0)throw new Error('public offset intrinsics were called');
          } finally {
            Math.trunc=savedTrunc;Number.isFinite=savedFinite;
          }
          console.log('restored',Math.trunc===savedTrunc&&Number.isFinite===savedFinite);
        "#
        .replace("TRUNCATE_RESULT", truncate)
        .replace("FINITE_RESULT", finite)
        .replace("PIXEL_FIXTURE", pixels);
        let out = run(&mut env, &mut host, &source);
        assert_eq!(out, ["size gl1 8", "size gl2 8", "1 0,255,0,255", "2 0,255,0,255", "restored true"], "{mode}");
        assert!(env.errors().is_empty(), "{mode}: {:?}", env.errors());
    }
}
