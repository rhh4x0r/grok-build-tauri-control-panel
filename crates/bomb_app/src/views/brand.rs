//! Backend brand marks (SVG, `currentColor`), used wherever a thread, service
//! or model is named. Colors follow the vendors' identities in dark mode and
//! drop a step in light mode for contrast.

use gpui_kit::*;

use crate::theme::Ui;

pub fn mark_path(backend: &str) -> Option<&'static str> {
    match backend {
        "grok" => Some("assets/icons/grok-mark.svg"),
        "claude" => Some("assets/icons/claude-mark.svg"),
        "codex" => Some("assets/icons/openai-mark.svg"),
        _ => None,
    }
}

/// The mark at `size` px, tinted with the backend color (or muted when
/// `tinted` is false, as in Zeron's neutral sidebar rows).
pub fn brand_mark(backend: &str, size: f32, tinted: bool, ui: &Ui) -> AnyElement {
    let color = if tinted { ui.backend(backend) } else { ui.text_muted };
    match mark_path(backend) {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_shrink_0()
            .text_color(color)
            .into_any_element(),
        None => div()
            .size(px(size * 0.5))
            .rounded_full()
            .flex_shrink_0()
            .bg(color)
            .into_any_element(),
    }
}

/// Human-readable model name: `grok-4.6` → "Grok 4.6",
/// `claude-opus-4-1-20250805` → "Claude Opus 4.1", `gpt-5-codex` → "GPT-5 Codex".
pub fn pretty_model(id: &str) -> String {
    let raw: Vec<&str> = id.split(['-', '_']).filter(|t| !t.is_empty()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let t = raw[i];
        // drop 8-digit date stamps
        if t.len() == 8 && t.chars().all(|c| c.is_ascii_digit()) {
            i += 1;
            continue;
        }
        let is_num = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit() || c == '.');
        if is_num(t) {
            // join "4" "1" → "4.1"
            let mut v = t.to_string();
            while i + 1 < raw.len() && raw[i + 1].len() <= 2 && raw[i + 1].chars().all(|c| c.is_ascii_digit()) {
                v = format!("{v}.{}", raw[i + 1]);
                i += 1;
            }
            match out.last() {
                Some(last) if last == "GPT" || last == "o" => {
                    let l = out.pop().unwrap();
                    out.push(format!("{l}-{v}"));
                }
                _ => out.push(v),
            }
            i += 1;
            continue;
        }
        let lower = t.to_ascii_lowercase();
        let word = match lower.as_str() {
            "gpt" => "GPT".to_string(),
            "o1" | "o3" | "o4" => lower.clone(),
            "ai" => "AI".to_string(),
            _ => {
                let mut c = lower.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            }
        };
        out.push(word);
        i += 1;
    }
    if out.is_empty() {
        id.to_string()
    } else {
        out.join(" ")
    }
}

/// One-line blurb per known model id (Zeron shows one beside each name).
/// Unknown ids get none; the row then shows the raw id instead.
pub fn model_blurb(id: &str) -> Option<&'static str> {
    Some(match id {
        "grok-4.6" => "Latest Grok, best for agentic coding",
        "grok-4.5" => "Previous generation, fast and capable",
        "claude-fable-5-1" => "Newest Claude, Mythos-class",
        "claude-fable-5" => "Mythos-class, previous point release",
        "claude-opus-5" => "Frontier Opus for hard problems",
        "claude-opus-4-8" => "Deep reasoning for hard problems",
        "claude-sonnet-5" => "Balanced speed and intelligence",
        "claude-haiku-4-5" => "Fastest and most affordable Claude",
        "gpt-6-astra" => "Our most capable model",
        "gpt-5.6-sol" => "Latest frontier agentic coding model",
        "gpt-5.6-terra" => "Balanced agentic coding, default",
        "gpt-5.6-luna" => "Fast and affordable agentic coding",
        "gpt-5.5" => "Proven previous-generation model",
        "gpt-5.4" => "Older generation, still capable",
        "gpt-5.4-mini" => "Small and quick for simple tasks",
        "gpt-5.3-codex-spark" => "Low-latency coding model",
        "gpt-5-codex" => "Original Codex agent model",
        _ => return None,
    })
}

/// Reasoning levels a backend accepts, and whether we can actually apply
/// them through its ACP adapter.
pub fn effort_levels(backend: &str) -> (&'static [&'static str], bool) {
    match backend {
        "grok" => (&["low", "medium", "high"], true),
        "codex" => (&["minimal", "low", "medium", "high"], true),
        "claude" => (&["low", "medium", "high", "max"], true),
        _ => (&[], false),
    }
}

/// Full word for the reasoning level, as the picker lists it.
pub fn effort_label(e: &str) -> &'static str {
    match e {
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "X-High",
        "max" => "Max",
        _ => "",
    }
}

/// The level each CLI uses when none is set (marked "Default").
pub fn default_effort(backend: &str) -> &'static str {
    match backend {
        "grok" | "codex" | "claude" => "medium",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::pretty_model;

    #[test]
    fn names() {
        assert_eq!(pretty_model("grok-4.6"), "Grok 4.6");
        assert_eq!(pretty_model("claude-opus-4-1-20250805"), "Claude Opus 4.1");
        assert_eq!(pretty_model("claude-sonnet-4-5"), "Claude Sonnet 4.5");
        assert_eq!(pretty_model("gpt-5-codex"), "GPT-5 Codex");
        assert_eq!(pretty_model("o3"), "o3");
    }
}
