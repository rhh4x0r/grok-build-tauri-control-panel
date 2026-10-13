//! Native image clipboard and location actions shared by attachment renderers.
use crate::{
    models::app::{AppModelHandle, ToastKind},
    runtime::spawn_service,
};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::*;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub enum Source {
    File(PathBuf),
    Attachment(Arc<Image>),
}

pub fn menu(mut menu: PopupMenu, source: Source) -> PopupMenu {
    for (label, action) in [
        ("Copy image", COPY),
        ("Reveal in Finder", REVEAL),
        ("Reveal in file tree", FILE_TREE),
    ] {
        let source = source.clone();
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| run(source.clone(), action, cx)));
    }
    menu
}

pub const COPY: u8 = 0;
pub const REVEAL: u8 = 1;
pub const FILE_TREE: u8 = 2;

/// Copy the image, or reveal its file in Finder or the file tree (an attachment is written to a
/// temporary file first).
pub fn run(source: Source, action: u8, cx: &mut App) {
    spawn_service(
        cx,
        async move {
            let (image, path) = match source {
                Source::File(path) => {
                    let format = match path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "png" => ImageFormat::Png,
                        "jpg" | "jpeg" => ImageFormat::Jpeg,
                        "gif" => ImageFormat::Gif,
                        "webp" => ImageFormat::Webp,
                        "svg" => ImageFormat::Svg,
                        "bmp" => ImageFormat::Bmp,
                        _ => return Err("Unsupported image format".to_string()),
                    };
                    let bytes = tokio::fs::read(&path).await.map_err(|e| e.to_string())?;
                    (Arc::new(Image::from_bytes(format, bytes)), path)
                }
                Source::Attachment(image) => {
                    let dir = std::env::temp_dir().join("bomb-code-images");
                    let path =
                        dir.join(format!("{}.{}", image.id(), image.format.extension()));
                    if action != 0 {
                        tokio::fs::create_dir_all(dir)
                            .await
                            .map_err(|e| e.to_string())?;
                        tokio::fs::write(&path, &image.bytes)
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                    (image, path)
                }
            };
            Ok((image, path))
        },
        move |result, cx| match result {
            Ok((image, path)) => match action {
                0 => cx.write_to_clipboard(ClipboardItem::new_image(&image)),
                1 => cx.reveal_path(&path),
                _ => {
                    let app = cx.global::<AppModelHandle>().0.clone();
                    app.update(cx, |m, cx| {
                        m.file_reveal_request = Some(path);
                        cx.notify();
                    });
                }
            },
            Err(error) => {
                let app = cx.global::<AppModelHandle>().0.clone();
                app.update(cx, |m, cx| {
                    m.toast(ToastKind::Error, error);
                    cx.notify();
                });
            }
        },
    );
}

/// Show an image large, over everything (`lightbox_layer`).
pub fn open(source: Source, cx: &mut App) {
    if let Some(app) = cx.try_global::<AppModelHandle>().map(|h| h.0.clone()) {
        app.update(cx, |m, cx| { m.lightbox = Some(source); cx.notify(); });
    }
}

pub fn close(cx: &mut App) {
    if let Some(app) = cx.try_global::<AppModelHandle>().map(|h| h.0.clone()) {
        app.update(cx, |m, cx| { m.lightbox = None; cx.notify(); });
    }
}

/// The open image, as large as the window allows, with its actions along the top. A click
/// outside it, ✕ or Escape closes it; right-click still offers the same menu.
pub fn lightbox_layer(source: Option<Source>) -> Option<AnyElement> {
    use crate::views::button::Button;
    use gpui_kit::assets::IconName as Lucide;
    use gpui_kit::component::button::ButtonVariants;
    use gpui_kit::component::Sizable;
    let source = source?;
    let name = match &source {
        Source::File(path) => path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        Source::Attachment(_) => "Image".to_string(),
    };
    let action = |id: &'static str, label: &'static str, icon: Lucide, what: u8| {
        let source = source.clone();
        Button::new(id).ghost().small().icon(icon).label(label).text_color(gpui_kit::white())
            .on_click(move |_, _, cx| { cx.stop_propagation(); run(source.clone(), what, cx); })
    };
    let picture = match &source {
        Source::File(path) => img(path.clone()),
        Source::Attachment(image) => img(image.clone()),
    };
    let menu_source = source.clone();
    Some(
        div()
            .id("lightbox")
            .absolute()
            .inset_0()
            .size_full()
            .occlude()
            .flex()
            .flex_col()
            .bg(gpui_kit::black().opacity(0.88))
            .on_click(|_, _, cx| close(cx))
            .child(
                div().id("lightbox-bar").flex().items_center().gap_2().px_4().py_3()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                        .text_size(px(crate::theme::Type::SMALL)).text_color(gpui_kit::white().opacity(0.8)).child(name))
                    .child(action("lightbox-copy", "Copy image", Lucide::Copy, COPY))
                    .child(action("lightbox-reveal", "Reveal in Finder", Lucide::Folder, REVEAL))
                    .child(action("lightbox-tree", "Reveal in file tree", Lucide::FolderOpen, FILE_TREE))
                    .child(Button::new("lightbox-close").ghost().small().icon(Lucide::X).text_color(gpui_kit::white()).tooltip("Close (Esc)")
                        .on_click(|_, _, cx| { cx.stop_propagation(); close(cx); })),
            )
            .child(
                // Only the picture keeps a click; the dark space around it closes.
                div().flex_1().min_h_0().flex().items_center().justify_center().p_6()
                    .child(
                        div().id("lightbox-image").max_w_full().max_h_full().flex()
                            .on_click(|_, _, cx| cx.stop_propagation())
                            .context_menu(move |m, _, _| menu(m, menu_source.clone()))
                            .child(picture.max_w_full().max_h_full().object_fit(ObjectFit::Contain).rounded(px(6.))),
                    ),
            )
            .into_any_element(),
    )
}
