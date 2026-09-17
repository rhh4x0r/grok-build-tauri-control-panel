//! Query provider-owned catalogs without sending a prompt or creating a Bomb Code thread.
use super::BackendInfo;
use crate::AppState;
use grok_acp::{AcpClient, AcpClientConfig, AvailableModel, ModelCatalog};
use grok_config::{Backend, GrokConfig, LaunchVia};
use std::{collections::HashMap, time::Duration};

pub async fn discover(state: &AppState, backend: Backend, config: &GrokConfig) -> BackendInfo {
    let descriptor = grok_config::descriptor(backend);
    let mut info = BackendInfo { id: backend.key().into(), display_name: descriptor.display_name.into(), available: false, via: None, reason: None,
        default_model: String::new(), models: Vec::new(), model_names: HashMap::new(), model_descriptions: HashMap::new(), model_error: None, commands: None, supports_headless: descriptor.supports_headless };
    let resolved = match grok_config::resolve_backend(backend, config) { Ok(value) => value, Err(error) => { info.reason = Some(error.to_string()); return info; } };
    info.available = true;
    info.via = Some(match resolved.via { LaunchVia::Binary => format!("binary:{}", resolved.program.display()), LaunchVia::Npx => "npx".into() });
    let result: Result<ModelCatalog, String> = async {
        if let Some(catalog) = state.registry.backend_model_catalog(backend).await { if catalog.commands.is_some() { return Ok(catalog); } }
        let cli_catalog = if backend == Backend::Grok {
            let models = state.grok_cli.list_models().await.map_err(|e|e.to_string())?;
            if models.is_empty() { return Err("The Grok CLI returned no models.".into()); }
            Some(ModelCatalog { commands: None, current: models.iter().find(|(_,default)| *default).map(|(id,_)|id.clone()),
                models: models.into_iter().map(|(id,_)|AvailableModel {name:id.clone(),id,description:None}).collect(), config_id: None })
        } else { None };
        let cwd = state.paths.panel_dir.join("model-discovery").join(backend.key());
        tokio::fs::create_dir_all(&cwd).await.map_err(|e|e.to_string())?;
        let mut launch = AcpClientConfig::new(resolved.program, cwd);
        launch.args = resolved.args;
        launch.backend_label = backend.key().into();
        launch.auth_preference = descriptor.auth_preference.iter().map(|s|s.to_string()).collect();
        launch.skip_auth_when_unadvertised = descriptor.skip_auth_when_unadvertised;
        launch.request_timeout = Duration::from_secs(30);
        launch.startup_timeout = Duration::from_secs(20);
        for key in descriptor.env_passthrough {
            if let Ok(value) = std::env::var(key) { launch.env.push((key.to_string(),value)); }
        }
        if let Some(provider) = config.backends.get(backend.key()) {
            for (key,value) in &provider.env { launch.env.retain(|(k,_)| k != key); launch.env.push((key.clone(),value.clone())); }
        }
        if backend == Backend::Claude && !launch.env.iter().any(|(key,_)| key == "ANTHROPIC_API_KEY") { launch.env.push(("ANTHROPIC_API_KEY".into(),String::new())); }
        let discovered = AcpClient::discover_models(launch).await.map_err(|e|e.to_string());
        if let Some(mut cli_catalog) = cli_catalog {
            // Grok CLI owns the model list; ACP owns the slash command list.
            if let Ok(acp) = discovered { cli_catalog.commands = acp.commands; }
            Ok(cli_catalog)
        } else { discovered }
    }.await;
    match result {
        Ok(catalog) if !catalog.models.is_empty() => {
            info.commands = catalog.commands;
            info.default_model = catalog.current.filter(|id| catalog.models.iter().any(|model| &model.id == id)).unwrap_or_else(||catalog.models[0].id.clone());
            for model in catalog.models { info.models.push(model.id.clone()); info.model_names.insert(model.id.clone(),model.name); if let Some(description)=model.description {info.model_descriptions.insert(model.id,description);} }
        }
        Ok(_) => info.model_error = Some("The provider did not expose any models. Check sign-in, then refresh.".into()),
        Err(error) => info.model_error = Some(format!("Could not load provider models: {error}")),
    }
    info
}
