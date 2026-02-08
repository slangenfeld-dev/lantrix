use std::{net::SocketAddr, path::{Path, PathBuf}};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "serveit", about = "Serve a directory over HTTP (with directory listings)")]
struct Args {
    #[arg(short = 'i', long = "interface", default_value = "127.0.0.1")]
    interface: String,

    #[arg(short = 'p', long = "port", default_value_t = 8080)]
    port: u16,

    #[arg(short = 'd', long = "dir")]
    dir: Option<PathBuf>,
}

#[derive(Clone)]
struct AppState {
    root: PathBuf, // canonicalized
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let root = args
        .dir
        .unwrap_or_else(|| std::env::current_dir().expect("failed to get current directory"))
        .canonicalize()
        .unwrap_or_else(|e| panic!("cannot canonicalize dir: {e}"));

    let addr: SocketAddr = format!("{}:{}", args.interface, args.port)
        .parse()
        .expect("invalid interface/port");

    println!("Serving: {}", root.display());
    println!("Listening on: http://{addr}");

    let app = Router::new()
        .route("/", get(serve_root))          // <-- no Path extractor
        .route("/*path", get(serve_path))     // <-- Path extractor
        .with_state(AppState { root });

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));

    axum::serve(listener, app).await.expect("server error");
}

async fn serve_root(State(state): State<AppState>) -> Response {
    serve_rel_path(state, "").await
}

async fn serve_path(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
) -> Response {
    serve_rel_path(state, &path).await
}

async fn serve_rel_path(state: AppState, rel: &str) -> Response {
    // URL decode (so "My%20File.txt" works)
    let decoded = match urlencoding::decode(rel) {
        Ok(s) => s.into_owned(),
        Err(_) => return (StatusCode::BAD_REQUEST, "Bad URL encoding").into_response(),
    };

    let candidate = state.root.join(&decoded);

    let meta = match tokio::fs::metadata(&candidate).await {
        Ok(m) => m,
        Err(_) => return (StatusCode::NOT_FOUND, "Not found").into_response(),
    };

    if meta.is_dir() {
        return list_dir(&state.root, &candidate).await;
    }

    match tokio::fs::read(&candidate).await {
        Ok(bytes) => {
            let mime = mime_guess::from_path(&candidate).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                .body(Body::from(bytes))
                .unwrap()
        }
        Err(_) => (StatusCode::FORBIDDEN, "Cannot read file").into_response(),
    }
}

async fn list_dir(root: &Path, dir: &Path) -> Response {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return (StatusCode::FORBIDDEN, "Cannot read directory").into_response(),
    };

    let mut items: Vec<(String, bool)> = Vec::new();
    while let Ok(Some(e)) = entries.next_entry().await {
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        items.push((name, is_dir));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));

    let mut html = String::new();
    html.push_str("<!doctype html><html><head><meta charset='utf-8'>");
    html.push_str("<title>Index</title>");
    html.push_str("<style>body{font-family:system-ui,Arial,sans-serif} a{text-decoration:none}</style>");
    html.push_str("</head><body>");
    html.push_str("<h1>Index</h1><ul>");

    if dir != root {
        html.push_str("<li><a href=\"../\">../</a></li>");
    }

    for (name, is_dir) in items {
        let display = if is_dir { format!("{}/", name) } else { name.clone() };
        let href = if is_dir {
            format!("{}{}", urlencoding::encode(&name), "/")
        } else {
            urlencoding::encode(&name).to_string()
        };

        html.push_str(&format!(
            "<li><a href=\"{href}\">{text}</a></li>",
            href = href,
            text = html_escape(&display)
        ));
    }

    html.push_str("</ul></body></html>");
    (StatusCode::OK, Html(html)).into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
