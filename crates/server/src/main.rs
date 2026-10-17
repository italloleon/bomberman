mod connection;
mod room;
mod rooms;

use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{Path, State, WebSocketUpgrade},
    response::Response,
    routing::{get, post},
};
use serde::Serialize;
use tracing_subscriber::EnvFilter;

use rooms::Rooms;

#[derive(Clone)]
struct AppState {
    rooms: Rooms,
    /// Goes into room links (`/r/<region>/<CODE>`) so more regions can exist later.
    region: String,
}

#[derive(Serialize)]
struct CreatedRoom {
    region: String,
    code: String,
}

async fn create_room(State(state): State<AppState>) -> Json<CreatedRoom> {
    let code = state.rooms.create();
    Json(CreatedRoom { region: state.region.clone(), code })
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Path(code): Path<String>,
    State(state): State<AppState>,
) -> Response {
    // Look the room up now, but upgrade even if it's missing: a browser can't read an
    // HTTP error body from a failed WebSocket handshake, so we explain over the socket.
    let room = state.rooms.get(&code.to_ascii_uppercase());
    ws.max_message_size(1024)
        .on_upgrade(move |socket| connection::run(socket, room))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let state = AppState {
        rooms: Rooms::default(),
        region: std::env::var("REGION").unwrap_or_else(|_| "nyc".into()),
    };

    let app = Router::new()
        .route("/api/rooms", post(create_room))
        .route("/ws/{code}", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state);

    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind port");
    tracing::info!("listening on {addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

/// Resolves on Ctrl+C, or SIGTERM (what `docker stop` sends). In a container the server
/// is PID 1, and Linux ignores SIGTERM for PID 1 unless the program handles it — without
/// this, every restart would hang until Docker gives up and kills it 10 s later.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        sigterm.recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
