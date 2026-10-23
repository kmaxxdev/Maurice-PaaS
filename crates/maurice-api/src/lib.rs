//! The panel's API. Generated types and service traits live in [`proto`];
//! the handlers are in [`services`]. Mount with [`router`].

pub mod proto {
    connectrpc::include_generated!();
}

pub mod crypto;
pub mod model_map;
pub mod services;
mod ui;

use std::sync::Arc;

/// Build the Connect router with every service registered.
pub fn router(state: Arc<services::State>) -> connectrpc::Router {
    connectrpc::Router::new()
        .add_service(Arc::new(services::WorkspacesSvc {
            state: state.clone(),
        }))
        .add_service(Arc::new(services::ServicesSvc {
            state: state.clone(),
        }))
        .add_service(Arc::new(services::SecretsSvc {
            state: state.clone(),
        }))
        .add_service(Arc::new(services::PlansSvc { state }))
}

/// Full axum application: Connect RPCs, a plain health route and the embedded SPA.
///
/// Connect methods live under `/maurice.v1.<Service>/<Method>`; everything else
/// is answered by the SPA (its assets, or `index.html` for client-side routes).
pub fn app(state: Arc<services::State>) -> axum::Router {
    let connect = router(state).into_axum_service();
    axum::Router::new()
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .route(
            "/maurice.v1.{service}/{method}",
            axum::routing::any_service(connect),
        )
        .fallback(ui::serve)
        .layer(tower_http::trace::TraceLayer::new_for_http())
}
