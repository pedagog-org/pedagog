//! The web UI's files, served beside the api: anything outside `/api` is a file
//! from `PEDAGOG_WEB_DIR` (gzipped copies when the browser takes them), else
//! `index.html`, so links into the app reload. Hashed assets are cached for
//! good, `index.html` never; every response carries the UI's security headers,
//! its CSP allowing `index.html`'s inline scripts by hash.

use std::path::Path;

use axum::Router;
use axum::extract::Request;
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, HeaderValue};
use axum::http::{HeaderName, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use sha2::{Digest, Sha256};
use tower_http::services::{ServeDir, ServeFile};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The files in `dir`, with `index.html` for anything else.
pub fn router(dir: &Path) -> anyhow::Result<Router> {
    let index = dir.join("index.html");
    let html = std::fs::read_to_string(&index)
        .map_err(|e| anyhow::anyhow!("the web UI's {}: {e}", index.display()))?;
    let csp = HeaderValue::from_str(&policy(&html))?;
    let files = ServeDir::new(dir)
        .precompressed_gzip()
        .fallback(ServeFile::new(&index).precompressed_gzip());
    Ok(Router::new()
        .fallback_service(files)
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            let csp = csp.clone();
            async move { headers(req, next, csp).await }
        })))
}

async fn headers(req: Request, next: Next, csp: HeaderValue) -> Response {
    let hashed = hashed_asset(req.uri().path());
    let mut res = next.run(req).await;
    let cache = if hashed && res.status().is_success() {
        IMMUTABLE
    } else {
        "no-cache"
    };
    let h = res.headers_mut();
    h.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(CONTENT_SECURITY_POLICY, csp);
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    res
}

/// Whether a path is one of trunk's content-hashed files
/// (`pedagog-web-3b3b0492b50115c4_bg.wasm`), safe to cache for good.
fn hashed_asset(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.split('.').next().unwrap_or(name);
    let stem = stem.strip_suffix("_bg").unwrap_or(stem);
    stem.rsplit_once('-')
        .is_some_and(|(_, hash)| hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The UI's Content-Security-Policy: only our own scripts (those inline in
/// `index.html` by hash, and WASM), styles, fonts and api. Inline `style`
/// attributes are allowed, as the framework sets some.
fn policy(index_html: &str) -> String {
    let hashes: String = inline_scripts(index_html)
        .map(|s| format!(" 'sha256-{}'", base64(&Sha256::digest(s.as_bytes()))))
        .collect();
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; \
         style-src 'self'; style-src-attr 'unsafe-inline'; font-src 'self'; \
         img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; \
         base-uri 'none'; form-action 'self'; object-src 'none'"
    )
}

/// The text of each `<script>` without a `src`.
fn inline_scripts(html: &str) -> impl Iterator<Item = &str> {
    html.split("<script").skip(1).filter_map(|rest| {
        let (attrs, body) = rest.split_once('>')?;
        if attrs.contains("src=") {
            return None;
        }
        Some(body.split_once("</script>")?.0)
    })
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_assets_are_trunks() {
        assert!(hashed_asset("/pedagog-web-3b3b0492b50115c4_bg.wasm"));
        assert!(hashed_asset("/pedagog-web-3b3b0492b50115c4.js"));
        assert!(hashed_asset("/tokens-ff0d9041cfff808e.css"));
        assert!(!hashed_asset("/index.html"));
        assert!(!hashed_asset("/courses/cop3502-f26"));
        assert!(!hashed_asset("/fonts/plex-sans-400.woff2"));
    }

    #[test]
    fn inline_scripts_are_allowed_by_hash() {
        let html = r#"<head><script>document.documentElement.classList.add('x')</script>
            <script type="module">import init from '/a.js';init();</script>
            <script src="/theme.js"></script></head>"#;
        let csp = policy(html);
        assert_eq!(csp.matches("'sha256-").count(), 2, "{csp}");
        // sha256("") in base64, a known value.
        assert_eq!(
            base64(&Sha256::digest(b"")),
            "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU="
        );
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"a"), "YQ==");
    }

    #[tokio::test]
    async fn files_deep_links_and_headers() {
        let dir = std::env::temp_dir().join(format!("pedagog-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<script>boot()</script>").unwrap();
        std::fs::write(dir.join("app-0123456789abcdef.js"), "x").unwrap();
        let app = router(&dir).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });

        let page = reqwest::get(format!("{url}/courses/cop3502-f26"))
            .await
            .unwrap();
        assert_eq!(page.status(), 200);
        assert_eq!(page.headers()["cache-control"], "no-cache");
        assert!(
            page.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("'sha256-")
        );
        assert_eq!(page.text().await.unwrap(), "<script>boot()</script>");
        let asset = reqwest::get(format!("{url}/app-0123456789abcdef.js"))
            .await
            .unwrap();
        assert_eq!(asset.headers()["cache-control"], IMMUTABLE);
        assert_eq!(asset.headers()["x-content-type-options"], "nosniff");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[sqlx::test(migrations = "../store/migrations")]
    async fn the_api_keeps_its_paths(pool: sqlx::PgPool) {
        let dir = std::env::temp_dir().join(format!("pedagog-web-api-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "app").unwrap();
        let (mut state, _) = crate::testing::state(pool);
        let mut config = (*state.config).clone();
        config.web_dir = Some(dir.clone());
        state.config = std::sync::Arc::new(config);
        let app = crate::http::router(state).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });

        assert_eq!(
            reqwest::get(format!("{url}/api/healthz"))
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok"
        );
        let missing = reqwest::get(format!("{url}/api/nope")).await.unwrap();
        assert_eq!(missing.status(), 404);
        assert!(
            missing
                .text()
                .await
                .unwrap()
                .contains("no such api endpoint")
        );
        assert_eq!(
            reqwest::get(format!("{url}/login"))
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "app"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
