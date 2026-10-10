//! Math in replies. Agents write LaTeX: `\[ … \]` or `$$ … $$` for an equation on its own line,
//! `\( … \)` or `$ … $` inside a sentence. Markdown knows none of it (it even eats the `\[`), so
//! replies are split here first: equations become their own parts for the screen to typeset
//! (MathJax on the Mac, SwiftMath on the iPhone), and math inside a sentence is turned into
//! readable text (`\frac{a}{b}` → a/b, `\pi` → π, `x^2` → x²), since a picture can't sit in a line.
//! Code (fenced blocks and `inline code`) is left exactly as written.

/// A piece of a reply.
#[derive(Debug, Clone, PartialEq)]
pub enum MathPart {
    /// Markdown, with any math inside sentences already made readable.
    Text(String),
    /// An equation on its own line, as TeX (without delimiters).
    Display(String),
}

/// Split a reply into markdown and equations. A reply with no math comes back as one `Text` equal
/// to the input. An equation still missing its closing delimiter (mid-stream) stays text.
pub fn split(text: &str) -> Vec<MathPart> {
    let mut parts = Vec::new();
    let mut prose = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        // Fenced code is copied untouched.
        if at_line_start(text, rest) && (rest.starts_with("```") || rest.starts_with("~~~")) {
            let fence = &rest[..3];
            let end = rest[3..].find(&format!("\n{fence}")).map(|i| 3 + i + 1 + 3);
            let end = end.map(|e| e + rest[e..].find('\n').map(|n| n + 1).unwrap_or(rest.len() - e)).unwrap_or(rest.len());
            prose.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        if rest.starts_with('`') {
            let ticks = rest.chars().take_while(|c| *c == '`').count();
            let marker = &rest[..ticks];
            if let Some(close) = rest[ticks..].find(marker) {
                let end = ticks + close + ticks;
                prose.push_str(&rest[..end]);
                rest = &rest[end..];
                continue;
            }
        }
        let display = [("\\[", "\\]"), ("$$", "$$")]
            .iter()
            .find_map(|(open, close)| rest.strip_prefix(open).and_then(|body| body.find(close).map(|end| (open.len(), end, close.len()))));
        if let Some((open, end, close)) = display {
            let tex = rest[open..open + end].trim();
            if !tex.is_empty() {
                flush(&mut parts, &mut prose);
                parts.push(MathPart::Display(tex.to_string()));
                rest = &rest[open + end + close..];
                continue;
            }
        }
        if let Some(body) = rest.strip_prefix("\\(") {
            if let Some(end) = body.find("\\)") {
                prose.push_str(&readable(&body[..end]));
                rest = &body[end + 2..];
                continue;
            }
        }
        if let Some((tex, used)) = dollar_inline(rest) {
            prose.push_str(&readable(tex));
            rest = &rest[used..];
            continue;
        }
        let ch = rest.chars().next().unwrap();
        prose.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    flush(&mut parts, &mut prose);
    if parts.is_empty() { parts.push(MathPart::Text(String::new())); }
    parts
}

/// Whether a reply has an equation to typeset (cheap check before `split`).
pub fn has_display_math(text: &str) -> bool {
    (text.contains("\\[") && text.contains("\\]")) || text.matches("$$").count() >= 2
}

fn flush(parts: &mut Vec<MathPart>, prose: &mut String) {
    if !prose.is_empty() { parts.push(MathPart::Text(std::mem::take(prose))); }
}

fn at_line_start(all: &str, rest: &str) -> bool {
    let at = all.len() - rest.len();
    at == 0 || all[..at].ends_with('\n')
}

/// `$x^2$` inside a sentence, but not prices: no space just inside either `$`, no newline, and
/// the closing `$` not followed by a digit. Returns the TeX and how many bytes it used.
fn dollar_inline(rest: &str) -> Option<(&str, usize)> {
    let body = rest.strip_prefix('$')?;
    if body.starts_with('$') || body.starts_with(char::is_whitespace) { return None; }
    let end = body.find('$')?;
    let tex = &body[..end];
    if tex.is_empty() || tex.contains('\n') || tex.ends_with(char::is_whitespace) { return None; }
    if body[end + 1..].starts_with(|c: char| c.is_ascii_digit()) { return None; }
    Some((tex, 1 + end + 1))
}

/// TeX made readable as plain text, for math inside a sentence.
pub fn readable(tex: &str) -> String {
    let mut out = String::new();
    let mut rest = tex.trim();
    while let Some(ch) = rest.chars().next() {
        match ch {
            '\\' => {
                let name: String = rest[1..].chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                if name.is_empty() {
                    // `\,` `\;` `\!` `\{` …: spacing or an escaped character.
                    let next = rest[1..].chars().next();
                    match next {
                        Some(',' | ';' | ':' | ' ') => out.push(' '),
                        Some('!') => {}
                        Some(c) => out.push(c),
                        None => {}
                    }
                    rest = &rest[1 + next.map(char::len_utf8).unwrap_or(0)..];
                    continue;
                }
                rest = &rest[1 + name.len()..];
                match name.as_str() {
                    "frac" | "dfrac" | "tfrac" => {
                        let (a, after) = group(rest);
                        let (b, after) = group(after);
                        rest = after;
                        out.push_str(&format!("{}/{}", wrap(&readable(a)), wrap(&readable(b))));
                    }
                    "sqrt" => {
                        let (a, after) = group(rest);
                        rest = after;
                        out.push_str(&format!("√{}", wrap(&readable(a))));
                    }
                    "text" | "mathrm" | "mathbf" | "mathit" | "operatorname" | "mathbb" | "mathcal" | "boldsymbol" => {
                        let (a, after) = group(rest);
                        rest = after;
                        let inner = readable(a);
                        out.push_str(&if name == "mathbb" { blackboard(&inner) } else { inner });
                    }
                    "left" | "right" | "big" | "Big" | "bigl" | "bigr" | "displaystyle" => {}
                    other => out.push_str(symbol(other).unwrap_or(other)),
                }
            }
            '^' | '_' => {
                let (a, after) = group(&rest[1..]);
                rest = after;
                let inner = readable(a);
                let mapped: Option<String> = inner.chars().map(|c| if ch == '^' { superscript(c) } else { subscript(c) }).collect();
                match mapped {
                    Some(s) => out.push_str(&s),
                    // No raised form for every character: `^(…)`, bracketed when longer than one.
                    None => {
                        let shown = if inner.chars().count() > 1 { format!("({inner})") } else { inner };
                        out.push_str(&format!("{ch}{shown}"));
                    }
                }
            }
            '{' | '}' => rest = &rest[1..],
            '-' => { out.push('−'); rest = &rest[1..]; }
            _ => { out.push(ch); rest = &rest[ch.len_utf8()..]; }
        }
    }
    out
}

/// One argument: `{…}` (balanced) or a single character/command.
fn group(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    if let Some(body) = s.strip_prefix('{') {
        let mut depth = 1;
        for (i, c) in body.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => { depth -= 1; if depth == 0 { return (&body[..i], &body[i + 1..]); } }
                _ => {}
            }
        }
        return (body, "");
    }
    if let Some(cmd) = s.strip_prefix('\\') {
        let len = 1 + cmd.chars().take_while(|c| c.is_ascii_alphabetic()).count().max(1);
        return (&s[..len.min(s.len())], &s[len.min(s.len())..]);
    }
    match s.chars().next() {
        Some(c) => (&s[..c.len_utf8()], &s[c.len_utf8()..]),
        None => ("", ""),
    }
}

/// Parentheses around anything longer than one symbol, so a/b stays unambiguous.
fn wrap(s: &str) -> String {
    if s.chars().count() <= 1 || s.chars().all(|c| c.is_alphanumeric()) { s.to_string() } else { format!("({s})") }
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α", "beta" => "β", "gamma" => "γ", "delta" => "δ", "epsilon" | "varepsilon" => "ε",
        "zeta" => "ζ", "eta" => "η", "theta" | "vartheta" => "θ", "iota" => "ι", "kappa" => "κ",
        "lambda" => "λ", "mu" => "μ", "nu" => "ν", "xi" => "ξ", "pi" => "π", "rho" => "ρ",
        "sigma" => "σ", "tau" => "τ", "upsilon" => "υ", "phi" | "varphi" => "φ", "chi" => "χ",
        "psi" => "ψ", "omega" => "ω", "Gamma" => "Γ", "Delta" => "Δ", "Theta" => "Θ", "Lambda" => "Λ",
        "Xi" => "Ξ", "Pi" => "Π", "Sigma" => "Σ", "Phi" => "Φ", "Psi" => "Ψ", "Omega" => "Ω",
        "cdot" => "·", "times" => "×", "div" => "÷", "pm" => "±", "mp" => "∓", "ast" => "∗",
        "le" | "leq" => "≤", "ge" | "geq" => "≥", "ne" | "neq" => "≠", "approx" => "≈", "equiv" => "≡",
        "sim" => "∼", "simeq" => "≃", "cong" => "≅", "propto" => "∝",
        "infty" => "∞", "partial" => "∂", "nabla" => "∇", "sum" => "∑", "prod" => "∏", "int" => "∫",
        "oint" => "∮", "in" => "∈", "notin" => "∉", "subset" => "⊂", "subseteq" => "⊆", "supset" => "⊃",
        "cup" => "∪", "cap" => "∩", "emptyset" | "varnothing" => "∅", "forall" => "∀", "exists" => "∃",
        "to" | "rightarrow" => "→", "leftarrow" => "←", "Rightarrow" | "implies" => "⇒", "iff" | "Leftrightarrow" => "⇔",
        "mapsto" => "↦", "langle" => "⟨", "rangle" => "⟩", "ldots" | "dots" | "cdots" => "…",
        "circ" => "∘", "otimes" => "⊗", "oplus" => "⊕", "wedge" => "∧", "vee" => "∨", "neg" | "lnot" => "¬",
        "mid" => "|", "vert" => "|", "lVert" | "rVert" | "Vert" => "‖", "lvert" | "rvert" => "|",
        "quad" | "qquad" => " ", "sin" => "sin", "cos" => "cos", "tan" => "tan", "log" => "log",
        "ln" => "ln", "exp" => "exp", "lim" => "lim", "max" => "max", "min" => "min", "det" => "det",
        "Re" => "Re", "Im" => "Im", "hbar" => "ℏ", "ell" => "ℓ", "dagger" => "†", "star" => "⋆",
        _ => return None,
    })
}

fn blackboard(s: &str) -> String {
    s.chars().map(|c| match c { 'R' => 'ℝ', 'C' => 'ℂ', 'N' => 'ℕ', 'Z' => 'ℤ', 'Q' => 'ℚ', 'H' => 'ℍ', 'P' => 'ℙ', other => other }).collect()
}

fn superscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰', '1' => '¹', '2' => '²', '3' => '³', '4' => '⁴', '5' => '⁵', '6' => '⁶', '7' => '⁷', '8' => '⁸', '9' => '⁹',
        '+' => '⁺', '−' | '-' => '⁻', '=' => '⁼', '(' => '⁽', ')' => '⁾', 'n' => 'ⁿ', 'i' => 'ⁱ', 'T' => 'ᵀ', '*' | '∗' => '*',
        _ => return None,
    })
}

fn subscript(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀', '1' => '₁', '2' => '₂', '3' => '₃', '4' => '₄', '5' => '₅', '6' => '₆', '7' => '₇', '8' => '₈', '9' => '₉',
        '+' => '₊', '−' | '-' => '₋', '=' => '₌', '(' => '₍', ')' => '₎', 'i' => 'ᵢ', 'j' => 'ⱼ', 'k' => 'ₖ', 'n' => 'ₙ',
        'm' => 'ₘ', 'x' => 'ₓ', 't' => 'ₜ',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equations_are_split_out_and_inline_math_reads_plainly() {
        let parts = split("The map is\n\\[ s(d)=\\frac{1-di}{|1-di|}. \\]\nwhere $d\\in\\mathbb{R}$ and \\(x^2\\) grows.");
        assert_eq!(parts, vec![
            MathPart::Text("The map is\n".into()),
            MathPart::Display("s(d)=\\frac{1-di}{|1-di|}.".into()),
            MathPart::Text("\nwhere d∈ℝ and x² grows.".into()),
        ]);
        assert_eq!(split("$$e^{i\\pi}+1=0$$"), vec![MathPart::Display("e^{i\\pi}+1=0".into())]);
    }

    #[test]
    fn code_prices_and_unfinished_equations_are_left_alone() {
        let code = "Run `echo $HOME $PATH` then\n```\n\\[ not math \\]\n```\nIt costs $5 and $10.";
        assert_eq!(split(code), vec![MathPart::Text(code.into())]);
        // Mid-stream: the closing \] hasn't arrived yet.
        assert_eq!(split("Here: \\[ s(d)=\\frac{1"), vec![MathPart::Text("Here: \\[ s(d)=\\frac{1".into())]);
        assert!(!has_display_math("plain"));
        assert!(has_display_math("\\[x\\]"));
    }

    #[test]
    fn readable_math() {
        assert_eq!(readable("\\frac{1-di}{|1-di|}"), "(1−di)/(|1−di|)");
        assert_eq!(readable("e^{i\\pi}+1=0"), "e^(iπ)+1=0");
        assert_eq!(readable("x_1^2 \\le \\sqrt{2}"), "x₁² ≤ √2");
        assert_eq!(readable("\\alpha\\cdot\\beta"), "α·β");
    }
}
