//! Native image clipboard and location actions shared by attachment renderers.
use crate::{
    models::app::{AppModelHandle, ToastKind},
    runtime::spawn_service,
};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub enum Source {
    File(PathBuf),
    Attachment(Arc<Image>),
}

pub fn menu(mut menu: PopupMenu, source: Source) -> PopupMenu {
    for (label, action) in [
        ("Copy image", 0),
        ("Reveal in Finder", 1),
        ("Reveal in file tree", 2),
    ] {
        let source = source.clone();
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
            let source = source.clone();
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
        }));
    }
    menu
}
