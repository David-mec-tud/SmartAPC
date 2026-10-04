//! Same-origin packaged SPA serving, without redirecting API failures to HTML.
use axum::{
    body::Body,
    extract::{Extension, OriginalUri},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct FrontendAssets(pub Arc<PathBuf>);
impl FrontendAssets {
    pub fn locate(executable: &Path) -> Self {
        let path = std::env::var_os("SMARTAPC_UI_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let packaged = executable
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("ui");
                if packaged.join("index.html").is_file() {
                    packaged
                } else {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("frontend/dist")
                }
            });
        Self(Arc::new(path))
    }
}

pub(crate) async fn spa(
    Extension(assets): Extension<FrontendAssets>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let requested = uri.path().trim_start_matches('/');
    if requested == "api" || requested.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": "API route not found"})),
        )
            .into_response();
    }
    let relative = Path::new(requested);
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let is_asset = requested.starts_with("assets/") || relative.extension().is_some();
    let path = if is_asset {
        assets.0.join(relative)
    } else {
        assets.0.join("index.html")
    };
    let content_type = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
    {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "json" => "application/json",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    };
    match tokio::fs::read(path).await {
        Ok(bytes) => Response::builder().header(header::CONTENT_TYPE, content_type)
            .header(header::CACHE_CONTROL, if is_asset { "public, max-age=86400" } else { "no-cache" })
            .body(Body::from(bytes)).expect("static response"),
        Err(_) if is_asset => StatusCode::NOT_FOUND.into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, axum::Json(serde_json::json!({"error": "Frontend assets are missing. Run npm ci && npm run build in frontend, or package ui beside the executable / set SMARTAPC_UI_DIR."}))).into_response(),
    }
}
