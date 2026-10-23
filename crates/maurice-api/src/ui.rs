//! The web UI, embedded from `packages/web/dist`. In debug builds rust-embed
//! reads the folder at runtime, so `pnpm build` is enough to refresh it; in
//! release builds the files are compiled in.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../packages/web/dist"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let candidate = if path.is_empty() { "index.html" } else { path };
    match Assets::get(candidate) {
        Some(file) => file_response(candidate, file.data.into_owned()),
        None if candidate.contains('.') => (StatusCode::NOT_FOUND, "not found").into_response(),
        // Client-side route: hand it to the SPA.
        None => match Assets::get("index.html") {
            Some(index) => file_response("index.html", index.data.into_owned()),
            None => (
                StatusCode::SERVICE_UNAVAILABLE,
                "The web UI is not built. Run `pnpm install && pnpm build` and restart, or use the CLI.",
            )
                .into_response(),
        },
    }
}

fn file_response(name: &str, body: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let cache = if name == "index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        body,
    )
        .into_response()
}
