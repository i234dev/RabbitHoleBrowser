//! Spaced background tiles must leave gaps and preserve one oversized image.
use rabbithole_engine::{gfx::Canvas, net::Url, Engine, Viewport};

fn render(source: &str) -> Canvas {
    let mut engine = Engine::new_bundled_fonts_only();
    engine.scripting = false;
    let page = engine.load_html(source, Url::parse("https://example.test/").unwrap(), Viewport { width: 100., height: 80. });
    let mut canvas = Canvas::new(100, 80);
    engine.render(&page, &mut canvas, 0., 0.);
    canvas
}
fn rgb(canvas: &Canvas, x: u32, y: u32) -> u32 {
    canvas.data[(y * canvas.width + x) as usize] & 0xffffff
}

#[test]
fn spaced_linear_and_radial_gradients_keep_real_tiles_and_gaps() {
    let linear = render(include_str!("../../../tests/pages/background-space-linear.html"));
    assert_eq!(rgb(&linear, 5, 5), 0x008000);
    assert_eq!(rgb(&linear, 15, 5), 0x0000ff);
    assert_eq!(rgb(&linear, 22, 5), 0xffff00);
    assert_eq!(rgb(&linear, 30, 5), 0x008000);
    assert_eq!(rgb(&linear, 5, 25), 0xffff00);
    assert_eq!(rgb(&linear, 5, 35), 0x008000);
    let radial = render(include_str!("../../../tests/pages/background-space-radial.html"));
    assert_eq!(rgb(&radial, 5, 5), 0x008000);
    assert_eq!(rgb(&radial, 22, 5), 0xffff00);
    assert_eq!(rgb(&radial, 5, 25), 0xffff00);
    assert_eq!(rgb(&radial, 55, 35), 0x008000);
}

#[test]
fn space_ignores_position_only_on_the_axis_that_fits_multiple_tiles() {
    let canvas = render(include_str!("../../../tests/pages/background-space-mixed-axis.html"));
    assert_eq!(rgb(&canvas, 5, 5), 0xffff00);
    assert_eq!(rgb(&canvas, 5, 10), 0x008000);
    assert_eq!(rgb(&canvas, 15, 10), 0x0000ff);
    assert_eq!(rgb(&canvas, 22, 10), 0xffff00);
    assert_eq!(rgb(&canvas, 55, 10), 0x008000);
    assert_eq!(rgb(&canvas, 5, 30), 0xffff00);
}

#[test]
fn oversized_spaced_image_is_still_painted_once_and_clipped() {
    let canvas = render(include_str!("../../../tests/pages/background-space-oversized-image.html"));
    for (x,y) in [(0,0),(20,20),(39,39)] {
        assert_eq!(rgb(&canvas,x,y),0x008000);
    }
    assert_eq!(rgb(&canvas,45,20),0xffffff);
}
