# Why this copy exists

`gpui-base` 0.6.1 from crates.io, unchanged except for one fix in `src/text/inline_flow.rs`.

A paragraph containing inline code (or any styled span) is laid out as line fragments. The kit
sizes each fragment's box from the shaped text, then renders the fragment with a text element
that wraps using per-character advance widths. The two measurements can disagree by a fraction
of a pixel, in which case the element wraps the last word onto a second line inside its box,
drawn on top of the next line, and the paragraph's height comes out one line short.

The fix sets `white_space: nowrap` on each fragment's element: a fragment is already exactly one
line, so it must never wrap again. Remove this directory and the `[patch.crates-io]` entry in the
workspace `Cargo.toml` once an upstream release includes an equivalent fix.
