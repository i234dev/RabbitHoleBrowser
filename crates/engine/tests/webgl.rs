//! A WebGL triangle is painted by the browser, not just returned by readPixels.
use rabbithole_engine::net::Url;
use rabbithole_engine::{Engine, Viewport};

#[test]
fn webgl_triangle_is_visible_in_the_page_frame() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let html = r#"<!doctype html><style>body{margin:0}</style><canvas id=draw width=64 height=64></canvas><script>
      const canvas=document.getElementById('draw');const gl=canvas.getContext('webgl');
      const vs=gl.createShader(gl.VERTEX_SHADER);gl.shaderSource(vs,'attribute vec2 p;void main(){gl_Position=vec4(p,0.0,1.0);}');gl.compileShader(vs);
      const fs=gl.createShader(gl.FRAGMENT_SHADER);gl.shaderSource(fs,'precision mediump float;void main(){gl_FragColor=vec4(1.0,0.0,0.0,1.0);}');gl.compileShader(fs);
      const program=gl.createProgram();gl.attachShader(program,vs);gl.attachShader(program,fs);gl.linkProgram(program);gl.useProgram(program);
      const buffer=gl.createBuffer();gl.bindBuffer(gl.ARRAY_BUFFER,buffer);gl.bufferData(gl.ARRAY_BUFFER,new Float32Array([-1,-1,1,-1,0,1]),gl.STATIC_DRAW);
      const position=gl.getAttribLocation(program,'p');gl.vertexAttribPointer(position,2,gl.FLOAT,false,0,0);gl.enableVertexAttribArray(position);gl.drawArrays(gl.TRIANGLES,0,3);
    </script>"#;
    let mut page = engine.load_html(
        html,
        Url::parse("https://example.test/").unwrap(),
        Viewport {
            width: 80.,
            height: 80.,
        },
    );
    engine.restyle(&mut page);
    let frame = engine.snapshot(&page);
    let index = (32 * frame.width as usize + 32) * 4;
    assert_eq!(&frame.rgba[index..index + 4], &[255, 0, 0, 255]);
    if let Ok(path) = std::env::var("RH_WEBGL_SCREENSHOT") {
        frame.save_png(std::path::Path::new(&path)).unwrap();
    }
    assert!(
        page.images
            .contains_key(&page.doc.element_by_id("draw").unwrap()),
        "GL pixels must reach the page canvas image"
    );
}

#[test]
fn webgl_local_demo_renders_triangle_and_texture() {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.loader.load_images = false;
    let html = include_str!("../../../tests/pages/webgl.html");
    let mut page = engine.load_html(
        html,
        Url::parse("https://example.test/").unwrap(),
        Viewport {
            width: 640.,
            height: 480.,
        },
    );
    engine.restyle(&mut page);
    let triangle = page.doc.element_by_id("triangle").unwrap();
    let texture = page.doc.element_by_id("texture").unwrap();
    let first = page
        .images
        .get(&triangle)
        .expect("triangle drawing buffer must be presented");
    let second = page
        .images
        .get(&texture)
        .expect("texture drawing buffer must be presented");
    assert!(first
        .pixels()
        .chunks_exact(4)
        .any(|p| p[0] > 100 && p[1] > 30 && p[2] > 30));
    assert!(second
        .pixels()
        .chunks_exact(4)
        .any(|p| p == [255, 80, 100, 255]));
    assert!(second
        .pixels()
        .chunks_exact(4)
        .any(|p| p == [50, 110, 240, 255]));
    if let Ok(path) = std::env::var("RH_WEBGL_DEMO_SCREENSHOT") {
        engine
            .snapshot(&page)
            .save_png(std::path::Path::new(&path))
            .unwrap();
    }
    let errors: Vec<_> = page
        .take_console()
        .into_iter()
        .filter(|(level, _)| level == "error")
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}
