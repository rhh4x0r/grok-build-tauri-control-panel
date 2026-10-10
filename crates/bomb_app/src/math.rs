//! Equations in replies, typeset on this Mac with MathJax (run by a pure-Rust JavaScript engine;
//! no browser, no network). One worker thread owns MathJax; each equation is drawn once per colour
//! and kept. The first one warms MathJax up (about half a second), so that happens at start-up.
//! See `bomb_core::transcript_math` for how replies are split into text and equations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::*;

/// A typeset equation: the picture and its size in pixels.
#[derive(Clone)]
pub struct Equation {
    pub image: Arc<Image>,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    tex: String,
    /// The ink colour, `#rrggbb`.
    color: String,
}

pub struct MathCache {
    ready: HashMap<Key, Result<Equation, String>>,
    asked: HashSet<Key>,
    worker: std::sync::mpsc::Sender<Key>,
}
impl Global for MathCache {}

/// Body text size the equations match (pixels per em).
const FONT_PX: f32 = 16.0;

/// Start the worker and warm MathJax up, once at start-up.
pub fn start(cx: &mut App) {
    let (to_worker, jobs) = std::sync::mpsc::channel::<Key>();
    let (done, results) = async_channel::unbounded::<(Key, Result<String, String>)>();
    std::thread::Builder::new()
        .name("mathjax".into())
        .spawn(move || {
            let mathjax = mathjax_svg_rs::MathJax::new();
            let options = mathjax_svg_rs::Options::default();
            // Warm up: the first formula loads MathJax's fonts and tables.
            let _ = mathjax.render_tex("x", &options);
            while let Ok(key) = jobs.recv() {
                let svg = mathjax.render_tex(&key.tex, &options);
                if done.send_blocking((key, svg)).is_err() { return; }
            }
        })
        .ok();
    cx.set_global(MathCache { ready: HashMap::new(), asked: HashSet::new(), worker: to_worker });
    cx.spawn(async move |cx| {
        while let Ok((key, svg)) = results.recv().await {
            let equation = svg.and_then(|svg| picture(&svg, &key.color));
            cx.update(|cx| {
                cx.global_mut::<MathCache>().ready.insert(key, equation);
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

/// The equation, if it's ready: `None` while it's being typeset (asked for now if new),
/// `Some(Err)` if MathJax couldn't read it.
pub fn equation(tex: &str, color: Hsla, cx: &mut App) -> Option<Result<Equation, String>> {
    let cache = cx.try_global::<MathCache>()?;
    let key = Key { tex: tex.to_string(), color: hex(color) };
    if let Some(done) = cache.ready.get(&key) { return Some(done.clone()); }
    if !cache.asked.contains(&key) {
        let sent = cache.worker.send(key.clone()).is_ok();
        let cache = cx.global_mut::<MathCache>();
        if sent { cache.asked.insert(key); } else { cache.ready.insert(key, Err("MathJax isn't running.".into())); }
    }
    None
}

/// MathJax's SVG, inked in `color` and sized in pixels (MathJax sizes it in `ex`; its viewBox is
/// in thousandths of an em).
fn picture(svg: &str, color: &str) -> Result<Equation, String> {
    if svg.contains("data-mjx-error") || svg.contains("merror") {
        return Err("MathJax couldn't read this equation.".into());
    }
    let view_box = svg.split("viewBox=\"").nth(1).and_then(|s| s.split('"').next()).ok_or("no viewBox")?;
    let numbers: Vec<f32> = view_box.split_whitespace().filter_map(|n| n.parse().ok()).collect();
    let [_, _, w, h] = numbers[..] else { return Err("bad viewBox".into()) };
    let (width, height) = (w / 1000.0 * FONT_PX, h / 1000.0 * FONT_PX);
    let mut svg = svg.replace("currentColor", color);
    // Pixel size in place of `ex` (the renderer has no font to measure an ex against).
    for (name, value) in [("width", width), ("height", height)] {
        if let Some(start) = svg.find(&format!(" {name}=\"")) {
            let from = start + name.len() + 3;
            if let Some(len) = svg[from..].find('"') { svg.replace_range(from..from + len, &format!("{value:.2}px")); }
        }
    }
    Ok(Equation { image: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())), width, height })
}

fn hex(color: Hsla) -> String {
    let rgb = color.to_rgb();
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(rgb.r), byte(rgb.g), byte(rgb.b))
}

#[cfg(test)]
mod tests {
    #[test]
    fn mathjax_svg_is_inked_and_sized_in_pixels() {
        let svg = r#"<svg style="vertical-align: -2.172ex;" xmlns="http://www.w3.org/2000/svg" width="15.122ex" height="5.271ex" viewBox="0 -1370 6684 2330"><g stroke="currentColor" fill="currentColor"></g></svg>"#;
        let eq = super::picture(svg, "#112233").unwrap();
        assert!((eq.width - 106.94).abs() < 0.1 && (eq.height - 37.28).abs() < 0.1);
        assert!(super::picture(r#"<svg data-mjx-error="x" viewBox="0 0 1 1"></svg>"#, "#000").is_err());
    }
}
