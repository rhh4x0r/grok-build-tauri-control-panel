//! Lazy directory browser; symlink directories are never traversed.
use crate::{runtime::spawn_service, theme::Ui};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    directory: bool,
}
pub struct FileTree {
    root: Option<PathBuf>,
    selected: Option<PathBuf>,
    children: HashMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    loading: HashSet<PathBuf>,
    errors: HashMap<PathBuf, String>,
    scroll: ScrollHandle,
    reveal_pending: bool,
}
impl FileTree {
    pub fn new() -> Self {
        Self {
            root: None,
            selected: None,
            children: HashMap::new(),
            expanded: HashSet::new(),
            loading: HashSet::new(),
            errors: HashMap::new(),
            scroll: ScrollHandle::new(),
            reveal_pending: false,
        }
    }
    pub fn reveal(&mut self, path: PathBuf, root: Option<PathBuf>, cx: &mut Context<Self>) {
        let root = root
            .filter(|r| path.starts_with(r))
            .or_else(|| path.parent().map(ToOwned::to_owned));
        self.children.clear();
        self.errors.clear();
        self.expanded.clear();
        self.root = root.clone();
        self.selected = Some(path.clone());
        self.reveal_pending = true;
        if let Some(root) = root {
            for parent in path
                .ancestors()
                .skip(1)
                .take_while(|p| p.starts_with(&root))
            {
                self.expanded.insert(parent.to_owned());
                self.load(parent.to_owned(), cx);
            }
        }
        cx.notify();
    }
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.is_none() {
            self.root = Some(root.clone());
            self.expanded.insert(root.clone());
            self.load(root, cx);
        }
    }
    fn load(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.children.contains_key(&path) || !self.loading.insert(path.clone()) {
            return;
        }
        let weak = cx.entity().downgrade();
        let dir = path.clone();
        // A server project's folders are listed by the server, relative to the folder this tree is rooted at.
        let remote = crate::runtime::servers(cx).for_root(&path.to_string_lossy());
        let base = self.root.clone().unwrap_or_else(|| path.clone());
        spawn_service(
            cx,
            async move {
                let mut entries = Vec::new();
                if let Some(remote) = remote {
                    let relative = dir.strip_prefix(&base).unwrap_or(std::path::Path::new("")).to_string_lossy().into_owned();
                    let listed = remote
                        .request("list_dir", serde_json::json!({ "root": remote.path_of(&base.to_string_lossy()), "path": relative }))
                        .await
                        .map_err(std::io::Error::other)?;
                    for item in listed.as_array().into_iter().flatten() {
                        entries.push(Entry { path: dir.join(item["name"].as_str().unwrap_or_default()), directory: item["dir"].as_bool().unwrap_or(false) });
                    }
                } else {
                    let mut reader = tokio::fs::read_dir(&dir).await?;
                    while let Some(e) = reader.next_entry().await? {
                        entries.push(Entry {
                            path: e.path(),
                            directory: e.file_type().await?.is_dir(),
                        });
                    }
                }
                entries.sort_by(|a, b| {
                    b.directory
                        .cmp(&a.directory)
                        .then_with(|| a.path.cmp(&b.path))
                });
                Ok::<_, std::io::Error>(entries)
            },
            move |result, cx| {
                let _ = weak.update(cx, |tree, cx| {
                    tree.loading.remove(&path);
                    match result {
                        Ok(entries) => {
                            tree.children.insert(path, entries);
                        }
                        Err(e) => {
                            tree.errors.insert(path, e.to_string());
                        }
                    }
                    cx.notify();
                });
            },
        );
    }
    fn rows(&self, path: &PathBuf, depth: usize, out: &mut Vec<(Entry, usize)>) {
        if let Some(entries) = self.children.get(path) {
            for e in entries {
                out.push((e.clone(), depth));
                if self.expanded.contains(&e.path) {
                    self.rows(&e.path, depth + 1, out);
                }
            }
        }
    }
}
impl Render for FileTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let mut rows = Vec::new();
        if let Some(root) = &self.root {
            self.rows(root, 0, &mut rows);
        }
        if self.reveal_pending {
            if let Some(index) = rows
                .iter()
                .position(|(e, _)| Some(&e.path) == self.selected.as_ref())
            {
                self.scroll.scroll_to_item(index);
                self.reveal_pending = false;
            }
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .child(
                div()
                    .text_size(px(crate::theme::Type::SMALL))
                    .text_color(ui.text_muted)
                    .whitespace_normal()
                    .child(
                        self.root
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "Choose a project to browse files.".into()),
                    ),
            )
            .child(
                div()
                    .id("file-tree-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows.into_iter().enumerate().map(|(i, (entry, depth))| {
                        let selected = self.selected.as_ref() == Some(&entry.path);
                        let open = self.expanded.contains(&entry.path);
                        let path = entry.path.clone();
                        div()
                            .id(("file", i))
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(28.))
                            .pl(px(8. + depth as f32 * 14.))
                            .pr_2()
                            .rounded(px(4.))
                            .text_size(px(crate::theme::Type::SMALL))
                            .cursor_pointer()
                            .when(selected, |el| el.bg(ui.hover))
                            .hover(move |el| el.bg(ui.hover))
                            .child(
                                Icon::from(if entry.directory {
                                    if open {
                                        Lucide::FolderOpen
                                    } else {
                                        Lucide::Folder
                                    }
                                } else {
                                    Lucide::File
                                })
                                .size(px(14.)),
                            )
                            .child(
                                div().overflow_hidden().text_ellipsis().child(
                                    entry
                                        .path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned(),
                                ),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(path.clone());
                                // A file: show it in Finder. A folder: open or close it here.
                                if !entry.directory { super::transcript::reveal_in_finder(&path); }
                                if entry.directory && !this.expanded.remove(&path) {
                                    this.expanded.insert(path.clone());
                                    this.load(path.clone(), cx);
                                }
                                cx.notify();
                            }))
                    })),
            )
            .when(!self.loading.is_empty(), |el| {
                el.child(div().text_size(px(crate::theme::Type::SMALL)).child("Loading files…"))
            })
            .children(self.errors.iter().map(|(p, e)| {
                div()
                    .text_size(px(crate::theme::Type::SMALL))
                    .text_color(ui.danger)
                    .child(format!("{}: {e}", p.display()))
            }))
    }
}
