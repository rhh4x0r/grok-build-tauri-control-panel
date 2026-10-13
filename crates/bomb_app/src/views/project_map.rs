//! The project as a map: the default branch runs across the top as a trunk, each thread is a card
//! branching off it (a thread that builds on another hangs below that one), and merged work curves
//! back into the trunk. Colour says where each thread stands; hovering traces its path back to main;
//! the selected card's side panel has every action the list view's cards have.

use crate::views::button::Button;
use crate::{models::app::AppModel, theme::Ui};
use bomb_core::services::project_overview::{describe_relation, ProjectOverview};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::{Disableable, Icon, Sizable};
use gpui_kit::{prelude::FluentBuilder as _, *};
use grok_persistence::WorkspaceRecord;
use std::collections::{HashMap, HashSet};

/// Map units (scaled by zoom): card size, spacing, and where the trunk runs.
const CARD_W: f32 = 236.;
const CARD_H: f32 = 104.;
const COLUMN: f32 = 284.;
const ROW: f32 = 152.;
const LEFT: f32 = 48.;
const TRUNK_Y: f32 = 36.;
const FIRST_ROW: f32 = 104.;
const MAP_HEIGHT: f32 = 560.;
const PANEL_W: f32 = 340.;

/// Which way the project page shows its threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapView {
    /// The map.
    #[default]
    Flat,
    /// The three columns.
    List,
}

/// What the map remembers between frames: view, pan and zoom, selection.
#[derive(Debug, Clone)]
pub struct MapState {
    pub view: MapView,
    pub zoom: f32,
    pub pan: Point<Pixels>,
    /// Branch of the selected card.
    pub selected: Option<String>,
    pub hover: Option<String>,
    pub show_merged: bool,
    /// A background drag in progress: where it started, and the pan then.
    pub drag: Option<(Point<Pixels>, Point<Pixels>)>,
}

impl Default for MapState {
    fn default() -> Self {
        Self {
            view: MapView::Flat, zoom: 1.0, pan: point(px(0.), px(0.)), selected: None, hover: None, show_merged: false, drag: None,
        }
    }
}

/// Where a thread stands, as the map colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// An agent is on it right now.
    Working,
    /// A thread with nothing saved yet.
    Fresh,
    /// Saved work not in the target yet, no PR.
    Ready,
    /// A pull request is open.
    Pr,
    /// Its work is in the target.
    Merged,
    /// A branch with no open thread that still has work of its own.
    Loose,
}

impl Stage {
    fn color(self, ui: &Ui) -> Hsla {
        match self {
            Stage::Working => ui.accent,
            Stage::Ready => ui.warning,
            // Its own colour: never confused with an agent at work (accent).
            Stage::Pr => hsla(188. / 360., 0.72, 0.5, 1.0),
            Stage::Merged => ui.success,
            Stage::Fresh | Stage::Loose => ui.text_muted,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Stage::Working => "working",
            Stage::Fresh => "fresh",
            Stage::Ready => "ready",
            Stage::Pr => "pr",
            Stage::Merged => "merged",
            Stage::Loose => "loose",
        }
    }
}

#[derive(Debug, Clone)]
struct PrBadge {
    number: u64,
    url: String,
    draft: bool,
    /// passing | failing | pending | none
    checks: &'static str,
}

/// One card's worth of facts.
#[derive(Debug, Clone)]
struct Node {
    branch: String,
    parent: Option<String>,
    stage: Stage,
    title: String,
    ahead: usize,
    behind: usize,
    base: String,
    files: Vec<String>,
    commits: Vec<(String, String)>,
    pr: Option<PrBadge>,
    workspace: Option<WorkspaceRecord>,
    agent: Option<String>,
    order: String,
}

/// A card to place: its branch, the branch it builds on, and its sort key along the trunk.
pub type Slot = (String, Option<String>, String);

/// Card positions (top-left, map units) for `(branch, parent branch, sort key)`. Threads without a
/// parent stand in columns along the trunk in sort order; a thread built on another hangs below it,
/// its first child in the parent's column and any others in new columns beside it.
pub fn layout(nodes: &[Slot]) -> HashMap<String, (f32, f32)> {
    let known: HashSet<&str> = nodes.iter().map(|(b, _, _)| b.as_str()).collect();
    let mut children: HashMap<&str, Vec<&Slot>> = HashMap::new();
    let mut roots = Vec::new();
    for n in nodes {
        match n.1.as_deref().filter(|p| known.contains(p) && *p != n.0) {
            Some(parent) => children.entry(parent).or_default().push(n),
            None => roots.push(n),
        }
    }
    roots.sort_by(|a, b| a.2.cmp(&b.2));
    for list in children.values_mut() {
        list.sort_by(|a, b| a.2.cmp(&b.2));
    }
    fn place<'a>(
        node: &'a Slot,
        depth: usize,
        column: &mut usize,
        children: &HashMap<&str, Vec<&'a Slot>>,
        out: &mut HashMap<String, (f32, f32)>,
        seen: &mut HashSet<&'a str>,
    ) {
        if !seen.insert(node.0.as_str()) { return; }
        out.insert(node.0.clone(), (LEFT + *column as f32 * COLUMN, FIRST_ROW + depth as f32 * ROW));
        for (i, child) in children.get(node.0.as_str()).into_iter().flatten().enumerate() {
            if i > 0 { *column += 1; }
            place(child, depth + 1, column, children, out, seen);
        }
    }
    let (mut out, mut seen, mut column) = (HashMap::new(), HashSet::new(), 0);
    for root in roots {
        place(root, 0, &mut column, &children, &mut out, &mut seen);
        column += 1;
    }
    // A cycle (a broken record) leaves nodes unplaced: give them columns of their own.
    for n in nodes {
        if !out.contains_key(&n.0) {
            out.insert(n.0.clone(), (LEFT + column as f32 * COLUMN, FIRST_ROW));
            column += 1;
        }
    }
    out
}

/// The branch and the branches it builds on, up to the trunk.
fn path_to_trunk(nodes: &[Node], from: &str) -> HashSet<String> {
    let mut path = HashSet::new();
    let mut at = Some(from.to_string());
    while let Some(branch) = at.filter(|b| path.insert(b.clone())) {
        at = nodes.iter().find(|n| n.branch == branch).and_then(|n| n.parent.clone());
    }
    path
}

fn checks_word(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        s if s.contains("fail") => "failing",
        s if s.contains("pend") => "pending",
        s if s.contains("pass") || s.contains("complete") => "passing",
        _ => "none",
    }
}

fn nodes(model: &Entity<AppModel>, overview: &ProjectOverview, root: &str, cx: &App) -> Vec<Node> {
    let m = model.read(cx);
    let base = overview.default_branch.clone();
    let workspace_for = |branch: &str| {
        m.workspaces.iter().find(|w| w.project_root == root && w.branch == branch && !w.inline && !w.shared_checkout && w.archived_at.is_none())
    };
    let branch_of = |workspace_id: &str| m.workspaces.iter().find(|w| w.id == workspace_id).map(|w| w.branch.clone());
    overview.branches.iter().filter(|b| b.name != base).map(|b| {
        let workspace = workspace_for(&b.name).cloned();
        let threads: Vec<_> = workspace.iter().flat_map(|w| w.threads.iter()).filter_map(|t| uuid::Uuid::parse_str(t).ok()).filter_map(|id| m.threads.get(&id)).collect();
        let working = threads.iter().any(|t| t.read(cx).thread.presence.turn_active());
        let agent = threads.first().map(|t| { let meta = &t.read(cx).meta; if meta.model.is_empty() { meta.backend.clone() } else { meta.model.clone() } });
        let parent = workspace.as_ref().and_then(|w| m.parent_link(&w.id)).and_then(|l| branch_of(&l.parent));
        let pr = match workspace.as_ref().and_then(|w| w.pr.clone()).filter(|pr| pr.state == "OPEN") {
            Some(pr) => Some(PrBadge { number: pr.number, url: pr.url, draft: pr.draft, checks: checks_word(&pr.checks) }),
            None => overview.prs.iter().find(|pr| pr.head_ref_name == b.name)
                .map(|pr| PrBadge { number: pr.number, url: pr.url.clone(), draft: pr.is_draft, checks: checks_word(pr.checks()) }),
        };
        let merged = workspace.as_ref().is_some_and(|w| w.merged.is_some());
        let stage = if working { Stage::Working }
            else if merged { Stage::Merged }
            else if pr.is_some() { Stage::Pr }
            else if b.ahead > 0 { if workspace.is_some() { Stage::Ready } else { Stage::Loose } }
            else if workspace.is_some() && b.behind == 0 { Stage::Fresh }
            else { Stage::Merged };
        Node {
            title: workspace.as_ref().map(|w| w.name.clone()).unwrap_or_else(|| b.name.clone()),
            order: workspace.as_ref().map(|w| w.created_at.clone()).unwrap_or_default(),
            branch: b.name.clone(), parent, stage, ahead: b.ahead, behind: b.behind, base: b.base.clone(),
            files: b.files.clone(), commits: b.commits.clone(), pr, workspace, agent,
        }
    }).collect()
}

/// One line saying where the thread stands.
fn headline(n: &Node) -> String {
    match (n.stage, &n.pr) {
        (Stage::Working, _) => "Agent working".into(),
        (Stage::Merged, _) => match n.workspace.as_ref().and_then(|w| w.merged.as_ref()).and_then(|m| m.pr) {
            Some(pr) => format!("Merged via PR #{pr}"),
            None => format!("In {}", n.base),
        },
        (Stage::Pr, Some(pr)) => format!("{}PR #{} · {}", if pr.draft { "Draft " } else { "" }, pr.number, match pr.checks { "none" => "no checks", c => c }),
        (Stage::Fresh, _) => "No saved work yet".into(),
        _ => describe_relation(n.ahead, n.behind, &n.base),
    }
}

/// The map, or the list, with the toolbar that switches between them. `list` renders the columns.
pub fn map_or_list(model: Entity<AppModel>, overview: &ProjectOverview, root: &str, list: AnyElement, ui: &Ui, cx: &App) -> AnyElement {
    let state = model.read(cx).project_map.clone();
    let all = nodes(&model, overview, root, cx);
    let merged_count = all.iter().filter(|n| n.stage == Stage::Merged).count();
    let toolbar = toolbar(&model, &state, merged_count, ui);
    if state.view == MapView::List {
        return div().flex().flex_col().gap_3().child(toolbar).child(list).into_any_element();
    }
    let shown: Vec<Node> = all.into_iter().filter(|n| state.show_merged || n.stage != Stage::Merged).collect();
    let selected = state.selected.as_ref().and_then(|b| shown.iter().find(|n| &n.branch == b)).cloned();
    let canvas = map_canvas(&model, &state, &shown, &overview.default_branch, ui);
    let body = div().flex().gap_3().h(px(MAP_HEIGHT))
        .child(canvas)
        .when_some(selected, |el, n| el.child(side_panel(&model, &n, root, ui, cx)));
    div().flex().flex_col().gap_3().child(toolbar).child(body).into_any_element()
}

fn toolbar(model: &Entity<AppModel>, state: &MapState, merged: usize, ui: &Ui) -> Div {
    let set = |model: &Entity<AppModel>, f: fn(&mut MapState)| {
        let model = model.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| model.update(cx, |m, cx| { f(&mut m.project_map); cx.notify(); })
    };
    let tab = |id: &'static str, label: &'static str, icon: Lucide, active: bool, f: fn(&mut MapState)| {
        let b = Button::new(id).small().icon(icon).label(label).on_click(set(model, f));
        if active { b.primary() } else { b.ghost() }
    };
    let legend = [(Stage::Working, "Agent working"), (Stage::Ready, "Ready to ship"), (Stage::Pr, "Pull request"), (Stage::Merged, "Merged")]
        .into_iter()
        .fold(div().flex().items_center().gap_3(), |el, (stage, label)| el.child(
            div().flex().items_center().gap_1().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_muted)
                .child(div().size(px(8.)).rounded_full().bg(stage.color(ui)))
                .child(label),
        ));
    div().flex().items_center().gap_2().flex_wrap()
        .child(tab("project-view-map", "Map", Lucide::GitBranch, state.view == MapView::Flat, |s| s.view = MapView::Flat))
        .child(tab("project-view-list", "List", Lucide::List, state.view == MapView::List, |s| s.view = MapView::List))
        .when(state.view != MapView::List, |el| el
            .child(div().w(px(1.)).h(px(16.)).mx_1().bg(ui.border))
            .child(legend)
            .child(div().flex_1())
            .when(merged > 0, |el| el.child(
                Button::new("project-map-merged").ghost().small()
                    .label(if state.show_merged { format!("Hide merged ({merged})") } else { format!("Show merged ({merged})") })
                    .on_click(set(model, |s| s.show_merged = !s.show_merged)),
            ))
            .child(Button::new("project-map-zoom-out").ghost().small().icon(Lucide::Minus).tooltip("Zoom out").on_click(set(model, |s| {
                s.zoom = (s.zoom - 0.15).max(0.5);
            })))
            .child(Button::new("project-map-zoom-in").ghost().small().icon(Lucide::Plus).tooltip("Zoom in").on_click(set(model, |s| {
                s.zoom = (s.zoom + 0.15).min(1.6);
            })))
            .child(Button::new("project-map-fit").ghost().small().label("Reset view").on_click(set(model, |s| {
                s.zoom = 1.0;
                s.pan = point(px(0.), px(0.));
            }))))

}

fn map_canvas(model: &Entity<AppModel>, state: &MapState, nodes: &[Node], trunk: &str, ui: &Ui) -> Stateful<Div> {
    let positions = layout(&nodes.iter().map(|n| (n.branch.clone(), n.parent.clone(), n.order.clone())).collect::<Vec<_>>());
    let lit: HashSet<String> = state.hover.as_deref().map(|h| path_to_trunk(nodes, h)).unwrap_or_default();
    let (zoom, pan) = (state.zoom, state.pan);
    let width = positions.values().map(|(x, _)| *x).fold(LEFT, f32::max) + CARD_W + 80.;

    // Lines first: the trunk, each thread's fork from it (or from the thread it builds on), merges back.
    let edges: Vec<(Point<f32>, Point<f32>, Hsla, f32)> = nodes.iter().filter_map(|n| {
        let (x, y) = *positions.get(&n.branch)?;
        let top = point(x + CARD_W / 2., y);
        let dim = !lit.is_empty() && !lit.contains(&n.branch);
        let weight = 1.5 + (n.ahead.min(24) as f32) / 8.;
        let color = n.stage.color(ui).opacity(if dim { 0.18 } else if lit.contains(&n.branch) { 1.0 } else { 0.6 });
        let from = match n.parent.as_ref().and_then(|p| positions.get(p)) {
            Some((px_, py)) => point(px_ + CARD_W / 2., py + CARD_H),
            None => point(x + CARD_W / 2. - 56., TRUNK_Y),
        };
        Some((from, top, color, weight))
    }).collect();
    let returns: Vec<(Point<f32>, Point<f32>, Hsla)> = nodes.iter().filter(|n| n.stage == Stage::Merged && n.parent.is_none()).filter_map(|n| {
        let (x, y) = *positions.get(&n.branch)?;
        let dim = !lit.is_empty() && !lit.contains(&n.branch);
        Some((point(x + CARD_W, y + CARD_H / 2.), point(x + CARD_W + 30., TRUNK_Y), ui.success.opacity(if dim { 0.15 } else { 0.5 })))
    }).collect();
    let trunk_color = ui.accent.opacity(if lit.is_empty() { 0.75 } else { 1.0 });
    let forks: Vec<f32> = nodes.iter().filter(|n| n.parent.is_none()).filter_map(|n| positions.get(&n.branch).map(|(x, _)| x + CARD_W / 2. - 56.)).collect();
    let bg_dot = ui.bg;
    let lines = canvas(move |bounds, _, _| bounds, move |_, bounds, window, _| {
        let at = |p: Point<f32>| bounds.origin + pan + point(px(p.x * zoom), px(p.y * zoom));
        // The trunk, with an arrowhead: main keeps going.
        let mut trunk_path = PathBuilder::stroke(px(3.));
        let (a, b) = (at(point(LEFT - 24., TRUNK_Y)), at(point(width, TRUNK_Y)));
        trunk_path.move_to(a);
        trunk_path.line_to(b);
        trunk_path.move_to(b + point(px(-7.), px(-5.)));
        trunk_path.line_to(b);
        trunk_path.line_to(b + point(px(-7.), px(5.)));
        if let Ok(p) = trunk_path.build() { window.paint_path(p, trunk_color); }
        for (from, to, color, weight) in &edges {
            let (s, e) = (at(*from), at(*to));
            let mid = (e.y - s.y) * 0.55;
            let mut path = PathBuilder::stroke(px(*weight * zoom.max(0.7)));
            path.move_to(s);
            path.cubic_bezier_to(e, point(s.x, s.y + mid), point(e.x, e.y - mid));
            if let Ok(p) = path.build() { window.paint_path(p, *color); }
        }
        for (from, to, color) in &returns {
            let (s, e) = (at(*from), at(*to));
            let mut path = PathBuilder::stroke(px(1.5 * zoom.max(0.7))).dash_array(&[px(4.), px(4.)]);
            path.move_to(s);
            path.cubic_bezier_to(e, point(e.x, s.y), point(e.x, e.y + px(20.)));
            if let Ok(p) = path.build() { window.paint_path(p, *color); }
        }
        // A dot on the trunk where each thread left it.
        for x in &forks {
            let c = at(point(*x, TRUNK_Y));
            let r = px(5. * zoom.max(0.7));
            window.paint_quad(fill(Bounds::new(c - point(r, r), size(r * 2., r * 2.)), trunk_color).corner_radii(r));
            let inner = r * 0.45;
            window.paint_quad(fill(Bounds::new(c - point(inner, inner), size(inner * 2., inner * 2.)), bg_dot).corner_radii(inner));
        }
    }).absolute().size_full();

    let mut area = div()
        .id("project-map")
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .rounded(px(12.))
        .border_1()
        .border_color(ui.border)
        .bg(Ui::alpha(ui.text, 0.015))
        .cursor(if state.drag.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
        .child(lines)
        .child(
            div().absolute().left(pan.x + px((LEFT - 24.) * zoom)).top(pan.y + px((TRUNK_Y - 26.) * zoom))
                .text_size(px(crate::theme::Type::SMALL)).font_weight(FontWeight::SEMIBOLD).text_color(ui.accent)
                .child(trunk.to_string()),
        );
    if nodes.is_empty() {
        area = area.child(
            div().absolute().left(px(LEFT)).top(px(FIRST_ROW)).text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint)
                .child("No threads yet. Start one with New thread: it branches off here."),
        );
    }
    for n in nodes {
        let Some((x, y)) = positions.get(&n.branch).copied() else { continue };
        let dim = !lit.is_empty() && !lit.contains(&n.branch);
        let selected = state.selected.as_deref() == Some(n.branch.as_str());
        area = area.child(card(model, n, selected, dim, point(px(x * zoom), px(y * zoom)) + pan, zoom, ui));
    }
    let (down, moved, up, out, wheel) = (model.clone(), model.clone(), model.clone(), model.clone(), model.clone());
    area
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| down.update(cx, |m, cx| {
            m.project_map.drag = Some((e.position, m.project_map.pan));
            cx.notify();
        }))
        .on_mouse_move(move |e: &MouseMoveEvent, _, cx| moved.update(cx, |m, cx| {
            if let (Some((start, from)), true) = (m.project_map.drag, e.dragging()) {
                m.project_map.pan = from + (e.position - start);
                cx.notify();
            }
        }))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| up.update(cx, |m, cx| { m.project_map.drag = None; cx.notify(); }))
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| out.update(cx, |m, cx| { m.project_map.drag = None; cx.notify(); }))
        .on_scroll_wheel(move |e: &ScrollWheelEvent, _, cx| {
            let d = e.delta.pixel_delta(px(20.));
            wheel.update(cx, |m, cx| {
                // ⌘ or Ctrl + scroll zooms; plain scroll pans.
                if e.modifiers.platform || e.modifiers.control {
                    m.project_map.zoom = (m.project_map.zoom - f32::from(d.y) / 400.).clamp(0.5, 1.6);
                } else {
                    m.project_map.pan += d;
                }
                cx.notify();
            });
            cx.stop_propagation();
        })
}

fn card(model: &Entity<AppModel>, n: &Node, selected: bool, dim: bool, at: Point<Pixels>, zoom: f32, ui: &Ui) -> AnyElement {
    let color = n.stage.color(ui);
    let compact = zoom < 0.62;
    let small = px(crate::theme::Type::SMALL);
    let caption = px(crate::theme::Type::CAPTION);
    let (pick, enter, leave) = (model.clone(), model.clone(), model.clone());
    let (branch, branch_in, branch_out) = (n.branch.clone(), n.branch.clone(), n.branch.clone());
    let details = [
        n.agent.clone(),
        (!n.files.is_empty()).then(|| format!("{} file{}", n.files.len(), if n.files.len() == 1 { "" } else { "s" })),
        (n.behind > 0 && n.stage != Stage::Merged).then(|| format!("{} newer on {}", n.behind, n.base)),
        n.workspace.is_none().then(|| "no open thread".to_string()),
    ].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let mut body = div()
        .id(SharedString::from(format!("map-card-{}", n.branch)))
        .absolute()
        .left(at.x)
        .top(at.y)
        .w(px(CARD_W * zoom))
        // A fixed height: lines meet the card's top and bottom edges exactly.
        .h(px(CARD_H * zoom))
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap_1()
        .p(px(10. * zoom.clamp(0.5, 1.3)))
        .rounded(px(10.))
        .border_1()
        .border_color(if selected { color } else { color.opacity(0.45) })
        .when(selected, |el| el.shadow_md())
        .bg(ui.bg)
        .cursor_pointer()
        // Clicking a card selects it; it never starts dragging the map.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, _, cx| pick.update(cx, |m, cx| {
            m.project_map.selected = if m.project_map.selected.as_deref() == Some(branch.as_str()) { None } else { Some(branch.clone()) };
            cx.notify();
        }))
        .on_hover(move |hovered, _, cx| {
            let (model, branch) = if *hovered { (enter.clone(), Some(branch_in.clone())) } else { (leave.clone(), None) };
            model.update(cx, |m, cx| {
                if *hovered || m.project_map.hover.as_deref() == Some(branch_out.as_str()) {
                    m.project_map.hover = branch;
                    cx.notify();
                }
            });
        })
        .child(
            div().flex().items_center().gap_1p5()
                .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(color))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(small).font_weight(FontWeight::MEDIUM).text_color(ui.text).child(n.title.clone())),
        );
    if !compact {
        body = body
            .child(div().text_size(small).text_color(if n.stage == Stage::Working { color } else { ui.text_muted }).overflow_hidden().text_ellipsis().whitespace_nowrap().child(headline(n)))
            .when(!details.is_empty(), |el| el.child(div().text_size(caption).text_color(ui.text_faint).overflow_hidden().text_ellipsis().whitespace_nowrap().child(details)))
            .when_some(n.pr.clone().filter(|_| n.stage != Stage::Pr), |el, pr| el.child(
                div().text_size(caption).text_color(ui.text_muted).child(format!("PR #{} · checks {}", pr.number, pr.checks)),
            ));
    }
    // A working thread breathes; a card that just changed stage fades in again. Opacity only:
    // the card is pinned to its spot on the map, so the animation must not move it.
    let id = SharedString::from(format!("map-{}-{}", n.branch, n.stage.key()));
    let opacity = if dim { 0.35 } else if n.stage == Stage::Merged { 0.7 } else { 1.0 };
    if n.stage == Stage::Working {
        body.opacity(opacity).with_animation(id, Animation::new(std::time::Duration::from_millis(1800)).repeat().with_easing(pulsating_between(0.35, 1.0)).with_max_fps(30.),
            move |el, t| el.border_color(color.opacity(t))).into_any_element()
    } else {
        body.with_animation(id, Animation::new(std::time::Duration::from_millis(350)).with_easing(crate::views::motion::ease_out_expo),
            move |el, t| el.opacity(opacity * t)).into_any_element()
    }
}

/// Everything about the selected thread, and everything you can do with it.
fn side_panel(model: &Entity<AppModel>, n: &Node, root: &str, ui: &Ui, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let busy = m.git_busy;
    let on_github = m.project_status.get(root).is_some_and(|s| s.remote);
    let small = px(crate::theme::Type::SMALL);
    let caption = px(crate::theme::Type::CAPTION);
    let section = |label: &str| div().pt_2().text_size(small).font_weight(FontWeight::SEMIBOLD).text_color(ui.text_muted).child(label.to_string());
    let close_model = model.clone();
    let mut panel = div()
        .id("project-map-panel")
        .w(px(PANEL_W))
        .flex_shrink_0()
        .h_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded(px(12.))
        .border_1()
        .border_color(ui.border)
        .bg(Ui::alpha(ui.text, 0.025))
        .child(
            div().flex().items_start().gap_2()
                .child(div().size(px(10.)).mt_1().flex_shrink_0().rounded_full().bg(n.stage.color(ui)))
                .child(div().flex_1().min_w_0().font_weight(FontWeight::SEMIBOLD).child(n.title.clone()))
                .child(Button::new("project-map-panel-close").ghost().small().icon(Lucide::X).tooltip("Close")
                    .on_click(move |_, _, cx| close_model.update(cx, |m, cx| { m.project_map.selected = None; cx.notify(); }))),
        )
        .child(div().text_size(caption).font_family(ui.mono.clone()).text_color(ui.text_faint).child(n.branch.clone()))
        .child(div().text_size(small).child(headline(n)))
        .when(n.stage != Stage::Merged && (n.ahead > 0 || n.behind > 0), |el| el.child(
            div().text_size(caption).text_color(ui.text_muted).child(describe_relation(n.ahead, n.behind, &n.base)),
        ))
        .when_some(n.parent.clone(), |el, p| el.child(
            div().flex().items_center().gap_1().text_size(caption).text_color(ui.text_muted)
                .child(Icon::from(Lucide::GitBranch).size(px(12.)))
                .child(format!("Builds on {p}")),
        ));

    let mut actions = div().flex().flex_wrap().gap_1p5();
    if let Some(w) = n.workspace.clone() {
        let (open_model, id) = (model.clone(), w.id.clone());
        actions = actions.child(Button::new("map-open-chat").primary().small().icon(Lucide::MessageSquare).label("Open chat")
            .on_click(move |_, _, cx| open_model.update(cx, |m, cx| m.open_workspace(id.clone(), cx))));
        let step = |key: &'static str, label: String, primary: bool, step: &'static str| {
            let (model, id) = (model.clone(), w.id.clone());
            let b = Button::new(key).small().label(label).disabled(busy).on_click(move |_, window, cx| super::project::confirm_ship(model.clone(), id.clone(), step, window, cx));
            if primary { b.primary() } else { b.outline() }
        };
        let merge = |key: &'static str, label: String, then_close: bool| {
            let (model, id) = (model.clone(), w.id.clone());
            Button::new(key).outline().small().label(label).disabled(busy)
                .on_click(move |_, window, cx| super::project::confirm_ship(model.clone(), id.clone(), if then_close { "merge_close" } else { "merge" }, window, cx))
        };
        match (n.stage, &n.pr) {
            (Stage::Working, _) => {}
            (Stage::Merged, _) => {}
            (Stage::Pr, Some(pr)) => {
                let url = pr.url.clone();
                actions = actions
                    .child(step("map-merge-pr", format!("Merge PR #{}", pr.number), false, "merge_pr"))
                    .child(step("map-push-pr", "Push latest work".into(), false, "push_pr"))
                    .when(pr.draft, |el| el.child(step("map-ready-pr", "Mark ready".into(), false, "ready_pr")))
                    .child(Button::new("map-view-pr").ghost().small().icon(Lucide::ExternalLink).label("View on GitHub").on_click(move |_, _, cx| cx.open_url(&url)));
            }
            _ if n.ahead > 0 && n.parent.is_some() => {
                actions = actions.child(Button::new("map-merge-wait").outline().small().disabled(true)
                    .label(format!("Ship {} first", n.parent.clone().unwrap_or_default()))
                    .tooltip("This thread builds on another. Once that lands, this one is brought up to date and can ship."));
            }
            _ if n.ahead > 0 => {
                if on_github {
                    actions = actions.child(step("map-open-pr", "Open pull request".into(), false, "open_pr"))
                        .child(step("map-open-draft", "Open draft PR".into(), false, "open_draft_pr"));
                }
                actions = actions.child(merge("map-merge", format!("Merge to {}", n.base), false))
                    .child(merge("map-merge-close", "Merge & close".into(), true));
            }
            _ => {}
        }
        let first_thread = w.threads.iter().find_map(|t| uuid::Uuid::parse_str(t).ok());
        if let (Some(thread), true) = (first_thread, n.stage != Stage::Working && n.behind > 0) {
            let model = model.clone();
            actions = actions.child(Button::new("map-update").ghost().small().icon(Lucide::RefreshCw).label(format!("Bring in latest {}", n.base))
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.sync_thread(thread, cx))));
        }
        if n.stage != Stage::Merged {
            let (model, branch) = (model.clone(), n.branch.clone());
            actions = actions.child(Button::new("map-new-from").ghost().small().icon(Lucide::Plus).label("New thread from here")
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.new_thread_from(branch.clone(), cx))));
        }
        if n.stage != Stage::Working {
            let (model, id, name) = (model.clone(), w.id.clone(), w.name.clone());
            let b = Button::new("map-close").small().label(if n.stage == Stage::Merged { "Close thread" } else { "Close" }).disabled(busy)
                .tooltip("Archive this thread: its folder is removed, the chat stays readable, and unmerged work is kept on its branch.")
                .on_click(move |_, window, cx| super::project::confirm_close(model.clone(), id.clone(), name.clone(), window, cx));
            actions = actions.child(if n.stage == Stage::Merged { b.primary() } else { b.ghost() });
        }
    } else {
        let (model, branch) = (model.clone(), n.branch.clone());
        actions = actions.child(Button::new("map-start-on").outline().small().icon(Lucide::Plus).label("Start a thread from this branch")
            .on_click(move |_, _, cx| model.update(cx, |m, cx| m.new_thread_from(branch.clone(), cx))));
        if let Some(pr) = &n.pr {
            let url = pr.url.clone();
            actions = actions.child(Button::new("map-view-pr").ghost().small().icon(Lucide::ExternalLink).label(format!("PR #{}", pr.number)).on_click(move |_, _, cx| cx.open_url(&url)));
        }
    }
    panel = panel.child(actions);

    if !n.commits.is_empty() && n.ahead > 0 {
        panel = panel.child(section("Latest saved changes"));
        for (_, title) in n.commits.iter().take(n.ahead.min(6)) {
            panel = panel.child(div().text_size(small).child(title.clone()));
        }
    }
    if !n.files.is_empty() {
        panel = panel.child(section(&format!("Files changed ({})", n.files.len())));
        for file in n.files.iter().take(12) {
            panel = panel.child(div().text_size(caption).font_family(ui.mono.clone()).text_color(ui.text_muted).overflow_hidden().text_ellipsis().whitespace_nowrap().child(file.clone()));
        }
        if n.files.len() > 12 {
            panel = panel.child(div().text_size(caption).text_color(ui.text_faint).child(format!("…and {} more", n.files.len() - 12)));
        }
    }
    panel.into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in gpui's own `test` macro.
    use super::{layout, Slot, COLUMN, FIRST_ROW, LEFT, ROW};

    fn n(branch: &str, parent: Option<&str>, order: &str) -> Slot {
        (branch.into(), parent.map(Into::into), order.into())
    }

    #[test]
    fn threads_stand_along_the_trunk_and_stacks_hang_below_their_parent() {
        let at = layout(&[
            n("b", None, "2"),
            n("a", None, "1"),
            n("a-child", Some("a"), "3"),
            n("a-second", Some("a"), "4"),
            n("grandchild", Some("a-child"), "5"),
        ]);
        // Oldest first along the trunk.
        assert_eq!(at["a"], (LEFT, FIRST_ROW));
        // A's first child under it, the second beside that, then B after A's whole family.
        assert_eq!(at["a-child"], (LEFT, FIRST_ROW + ROW));
        assert_eq!(at["grandchild"], (LEFT, FIRST_ROW + 2. * ROW));
        assert_eq!(at["a-second"], (LEFT + COLUMN, FIRST_ROW + ROW));
        assert_eq!(at["b"], (LEFT + 2. * COLUMN, FIRST_ROW));
    }

    #[test]
    fn unknown_parents_and_cycles_still_get_a_place() {
        let at = layout(&[n("x", Some("gone"), "1"), n("p", Some("q"), "2"), n("q", Some("p"), "3")]);
        assert_eq!(at.len(), 3);
        assert_eq!(at["x"], (LEFT, FIRST_ROW));
        let mut spots: Vec<_> = at.values().map(|(x, y)| ((*x) as i32, (*y) as i32)).collect();
        spots.sort();
        spots.dedup();
        assert_eq!(spots.len(), 3, "no two cards on top of each other");
    }
}
