//! The panel's API. Generated types and service traits live in [`proto`];
//! the handlers are in [`services`]. Mount with [`router`].

pub mod proto {
    connectrpc::include_generated!();
}

pub mod services;

use std::sync::Arc;

/// Build the Connect router with every service registered.
pub fn router(state: Arc<services::State>) -> connectrpc::Router {
    connectrpc::Router::new()
        .add_service(Arc::new(services::WorkspacesSvc {
            state: state.clone(),
        }))
        .add_service(Arc::new(services::PlansSvc { state }))
}

/// Full axum application: Connect RPCs plus a plain health route.
pub fn app(state: Arc<services::State>) -> axum::Router {
    axum::Router::new()
        .route("/healthz", axum::routing::get(|| async { "ok" }))
        .fallback_service(router(state).into_axum_service())
        .layer(tower_http::trace::TraceLayer::new_for_http())
}
