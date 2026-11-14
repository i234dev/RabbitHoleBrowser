use crate::tests::{run, setup};

#[test]
fn webgl_matrix4_validation_converts_webidl_inputs_before_null_noop_and_gl_errors() {
    let (mut env, mut host) = setup("<canvas id=a width=1 height=1></canvas><canvas id=b width=1 height=1></canvas>");
    let out = run(&mut env, &mut host, r#"
      for(const [id,version] of [['a',1],['b',2]]) {
        const gl=document.getElementById(id).getContext(version===2?'webgl2':'webgl',{preserveDrawingBuffer:true,premultipliedAlpha:false});
        const prefix=version===2?'#version 300 es\n':'';
        function shader(kind,source){const s=gl.createShader(kind);gl.shaderSource(s,source);gl.compileShader(s);if(!gl.getShaderParameter(s,gl.COMPILE_STATUS))throw new Error(gl.getShaderInfoLog(s));return s;}
        const p=gl.createProgram();
        gl.attachShader(p,shader(gl.VERTEX_SHADER,prefix+(version===2?'in':'attribute')+' vec2 position;void main(){gl_Position=vec4(position,0.0,1.0);}'));
        gl.attachShader(p,shader(gl.FRAGMENT_SHADER,prefix+'precision mediump float;'+(version===2?'out vec4 pixel;':'')+'uniform mat4 matrix;uniform vec4 color;void main(){'+(version===2?'pixel':'gl_FragColor')+'=matrix*color;}'));
        gl.linkProgram(p);if(!gl.getProgramParameter(p,gl.LINK_STATUS))throw new Error(gl.getProgramInfoLog(p));gl.useProgram(p);
        const b=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,b);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([-1,-1,3,-1,-1,3]),gl.STATIC_DRAW);
        const a=gl.getAttribLocation(p,'position');gl.vertexAttribPointer(a,2,gl.FLOAT,false,0,0);gl.enableVertexAttribArray(a);
        const matrix=gl.getUniformLocation(p,'matrix'),color=gl.getUniformLocation(p,'color');
        const good=[0.25,0.5,0.75,0,0,1,0,0,0,0,1,0,0,0,0,1];gl.uniformMatrix4fv(matrix,false,good);gl.uniform4f(color,1,0,0,1);
        function check(condition){if(!condition)throw new Error('matrix4 validation regression');}
        function pixel(){gl.drawArrays(gl.TRIANGLES,0,3);const v=new Uint8Array(4);gl.readPixels(0,0,1,1,gl.RGBA,gl.UNSIGNED_BYTE,v);return Array.from(v).join(',');}
        for(const [transpose,data,error] of [[false,[],gl.INVALID_VALUE],[false,new Float32Array(15),gl.INVALID_VALUE],[false,new Float32Array(17),gl.INVALID_VALUE],[false,new Float32Array(32),gl.INVALID_OPERATION],[true,good,gl.INVALID_VALUE]]) {
          gl.uniformMatrix4fv(matrix,transpose,data);check(gl.getError()===error);check(Array.from(gl.getUniform(p,matrix)).join(',')===good.join(','));check(pixel()==='64,128,191,255');
        }
        gl.uniformMatrix4fv(color,false,good);check(gl.getError()===gl.INVALID_OPERATION);check(Array.from(gl.getUniform(p,color)).join(',')==='1,0,0,1');
        gl.uniformMatrix4fv(null,true,[]);check(gl.getError()===gl.NO_ERROR);
        gl.uniformMatrix4fv(undefined,true,[]);check(gl.getError()===gl.NO_ERROR);
        function callableSequence(){}callableSequence[Symbol.iterator]=function(){return good[Symbol.iterator]();};
        gl.uniformMatrix4fv(matrix,false,callableSequence);check(gl.getError()===gl.NO_ERROR);check(Array.from(gl.getUniform(p,matrix)).join(',')===good.join(','));
        let iteratorReads=0;const once={get [Symbol.iterator](){++iteratorReads;return good[Symbol.iterator].bind(good);}};
        gl.uniformMatrix4fv(null,false,once);check(iteratorReads===1&&gl.getError()===gl.NO_ERROR);
        const iterate=function(){return good[Symbol.iterator]();};Object.defineProperty(iterate,'call',{get(){throw new Error('Iterator.call must not be read');}});
        const seq={ [Symbol.iterator]:iterate },oldApply=Reflect.apply;
        Reflect.apply=function(){throw new Error('Public Reflect.apply must not be read');};
        try{gl.uniformMatrix4fv(matrix,false,seq);gl.uniformMatrix4fv(null,false,seq);}finally{Reflect.apply=oldApply;}
        check(gl.getError()===gl.NO_ERROR&&Array.from(gl.getUniform(p,matrix)).join(',')===good.join(','));
        for(const location of [null,matrix]) {
          for(const data of [undefined,null,1,[Symbol()],Array(20).fill(0).concat([Symbol()])]) {
            let threw=false;try{gl.uniformMatrix4fv(location,true,data);}catch(e){threw=e instanceof TypeError;}check(threw);check(gl.getError()===gl.NO_ERROR);
          }
        }
        let converted=false;const data={ [Symbol.iterator](){converted=true;return [1][Symbol.iterator]();} };
        let badLocation=false;try{gl.uniformMatrix4fv({},false,data);}catch(e){badLocation=e instanceof TypeError;}check(badLocation&&!converted);check(gl.getError()===gl.NO_ERROR);
        let arity=false;try{gl.uniformMatrix4fv(null,false);}catch(e){arity=e instanceof TypeError;}check(arity);
        check(pixel()==='64,128,191,255');check(gl.getError()===gl.NO_ERROR);console.log(version,'matrix4 validation ok');
      }
    "#);
    assert_eq!(out, ["1 matrix4 validation ok", "2 matrix4 validation ok"]);
    assert!(env.errors().is_empty(), "{:?}", env.errors());
}
