//! Bomb Code — GPUI desktop app.

mod actions;
mod models;
mod runtime;
mod smoke;
mod theme;
mod views;

use std::sync::Arc;

use gpui_kit::*;

/// Serves files under `crates/bomb_app/assets` (logo, sprites) alongside the
/// gpui-kit icon bundle.
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(rest) = path.strip_prefix("assets/") {
            let data: Option<&'static [u8]> = match rest {
                "logo.png" => Some(include_bytes!("../assets/logo.png")),
                "logo-status.png" => Some(include_bytes!("../assets/logo-status.png")),
                "icon.png" => Some(include_bytes!("../assets/icon.png")),
                _ => None,
            };
            return Ok(data.map(std::borrow::Cow::Borrowed));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::AllAssets.list(path)
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bomb-tokio")
        .build()
        .expect("tokio runtime");
    let state = match rt.block_on(bomb_core::AppState::initialize()) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("Bomb Code failed to start: {e:#}");
            std::process::exit(1);
        }
    };
    let handle = rt.handle().clone();
    // The runtime must outlive the UI loop; `run` never returns on macOS.
    let _rt: &'static tokio::runtime::Runtime = Box::leak(Box::new(rt));

    gpui_kit::application().with_assets(Assets).run(move |cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        cx.set_global(runtime::Tokio(handle));
        cx.set_global(runtime::Services(state));
        actions::init(cx);

        let model = cx.new(|cx| models::app::AppModel::new(cx));
        runtime::start_bridge(cx, model.downgrade());
        cx.set_global(models::app::AppModelHandle(model.clone()));
        views::root::open_main_window(model.clone(), cx);
        smoke::maybe_run(model, cx);
        cx.activate(true);
    });
}
