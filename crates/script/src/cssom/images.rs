//! Computed CSS Images values from the same layers used for painting.
use super::{color_css, position_css, tokens_to_css};
use rabbithole_css::Token;
use rabbithole_style::{BackgroundImage, BackgroundLayer, ColorInterp, GradientDirection, GradientStop, HueMethod, InterpSpace, Length, RadialShape, RadialSize};

pub(super) fn layers(layers: &[BackgroundLayer]) -> String {
    if layers.is_empty() {
        return "none".into();
    }
    layers.iter().map(|layer| image(&layer.image)).collect::<Vec<_>>().join(", ")
}

fn degrees(radians: f32) -> String {
    format!("{}deg", (radians.to_degrees() * 100_000.0).round() / 100_000.0)
}

fn position(parts: &mut Vec<String>, (x, y): &(Length, Length)) {
    if !matches!((x,y), (Length::Percent(50.0), Length::Percent(50.0))) {
        parts.push(format!("at {} {}", position_css(x), position_css(y)));
    }
}

fn image(image: &BackgroundImage) -> String {
    let mut prelude = Vec::new();
    let (kind, stops, repeating, interp) = match image {
        BackgroundImage::None => return "none".into(),
        BackgroundImage::Url(url) => return format!("url({})", tokens_to_css(&[Token::String(url.clone())])),
        BackgroundImage::LinearGradient { direction, stops, repeating, interp } => {
            match direction {
                GradientDirection::Angle(a) if (*a - std::f32::consts::PI).abs() > f32::EPSILON => prelude.push(degrees(*a)),
                GradientDirection::Corner(x, y) if (*x,*y) != (0,1) => {
                    let mut words = vec!["to"];
                    if *x != 0 { words.push(if *x < 0 { "left" } else { "right" }); }
                    if *y != 0 { words.push(if *y < 0 { "top" } else { "bottom" }); }
                    prelude.push(words.join(" "));
                }
                _ => {}
            }
            ("linear", stops, repeating, interp)
        }
        BackgroundImage::RadialGradient { shape,size,position: center,stops,repeating,interp } => {
            match size {
                RadialSize::Explicit(x,y) => {
                    prelude.push(position_css(x));
                    if matches!(shape,RadialShape::Ellipse) { prelude.push(position_css(y)); }
                }
                _ => {
                    if matches!(shape,RadialShape::Circle) { prelude.push("circle".into()); }
                    let extent = match size {
                        RadialSize::ClosestSide => "closest-side",
                        RadialSize::FarthestSide => "farthest-side",
                        RadialSize::ClosestCorner => "closest-corner",
                        _ => "",
                    };
                    if !extent.is_empty() { prelude.push(extent.into()); }
                }
            }
            position(&mut prelude,center);
            ("radial", stops, repeating, interp)
        }
        BackgroundImage::ConicGradient { angle,position: center,stops,repeating,interp } => {
            if *angle != 0.0 { prelude.push(format!("from {}",degrees(*angle))); }
            position(&mut prelude,center);
            ("conic", stops, repeating, interp)
        }
    };
    let default = if stops.iter().any(|s| s.wide.is_some()) { InterpSpace::Oklab } else { InterpSpace::Srgb };
    if interp.space != default || interp.hue != HueMethod::Shorter {
        prelude.push(interpolation(*interp));
    }
    let mut args = Vec::with_capacity(stops.len() + 1);
    if !prelude.is_empty() { args.push(prelude.join(" ")); }
    args.extend(stops.iter().map(stop));
    let prefix = if *repeating { "repeating-" } else { "" };
    format!("{prefix}{kind}-gradient({})",args.join(", "))
}

fn interpolation(interp: ColorInterp) -> String {
    let hue = match interp.hue {
        HueMethod::Shorter => "",
        HueMethod::Longer => " longer hue",
        HueMethod::Increasing => " increasing hue",
        HueMethod::Decreasing => " decreasing hue",
    };
    format!("in {}{hue}",interp.space.css_name())
}

fn stop(stop: &GradientStop) -> String {
    let mut parts = Vec::new();
    if !stop.hint {
        parts.push(stop.components.as_ref().map_or_else(|| color_css(stop.color), |c| c.to_css()));
    }
    if let Some(p) = &stop.position { parts.push(position_css(p)); }
    parts.join(" ")
}
