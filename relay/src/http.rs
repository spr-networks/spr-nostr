//! HTTP front for the relay port.
//!
//! nostr-relay-builder 0.44.1's `LocalRelay::take_connection(stream, addr)`
//! feeds the stream straight into `async_wsocket::native::take_upgraded`, which
//! is `WebSocketStream::from_raw_socket(.., Role::Server, None)` — i.e. it does
//! NOT perform the HTTP/WebSocket handshake; it assumes the stream is already
//! upgraded. `run()` is therefore NOT used at all (matching the crate's own
//! `examples/hyper.rs`). Instead we bind the listener ourselves and, per
//! connection:
//!
//!   * peek the request head (non-destructively) to find its end, then consume
//!     exactly the header bytes;
//!   * a `GET` with `Accept: application/nostr+json` (and no Upgrade) → answer
//!     the NIP-11 document with permissive CORS;
//!   * a WebSocket upgrade → write the `101 Switching Protocols` handshake
//!     ourselves, then hand the now-upgraded raw stream to
//!     `LocalRelay::take_connection`;
//!   * `OPTIONS` → a CORS preflight response;
//!   * anything else → a short landing page.
//!
//! (A pure peek-then-`take_connection(raw_stream)` approach cannot work: since
//! `take_connection` skips the handshake, the client would never receive its
//! `101` response.)

use std::net::SocketAddr;
use std::time::Duration;

use base64::prelude::{Engine as _, BASE64_STANDARD};
use nostr::hashes::sha1::Hash as Sha1Hash;
use nostr::hashes::{Hash, HashEngine};
use nostr_relay_builder::LocalRelay;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const MAX_HEAD: usize = 16 * 1024;

/// What the incoming HTTP request is.
#[derive(Debug, PartialEq, Eq)]
pub enum ReqKind {
    /// WebSocket upgrade; carries the `Sec-WebSocket-Key`.
    WebSocket(String),
    /// NIP-11 relay information document request.
    Nip11,
    /// CORS preflight.
    Options,
    /// Anything else (serve a landing page).
    Other,
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    for line in head.split("\r\n").skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim());
            }
        }
    }
    None
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

/// Classify a raw HTTP request head (request line + headers, no body).
pub fn classify(head: &str) -> ReqKind {
    let request_line = head.split("\r\n").next().unwrap_or("");
    let method = request_line
        .split(' ')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();

    let is_upgrade = header(head, "upgrade")
        .map(|u| contains_ci(u, "websocket"))
        .unwrap_or(false)
        && header(head, "connection")
            .map(|c| contains_ci(c, "upgrade"))
            .unwrap_or(false);

    if is_upgrade {
        if let Some(key) = header(head, "sec-websocket-key") {
            return ReqKind::WebSocket(key.to_string());
        }
        // Upgrade without a key is malformed — fall through to Other.
    }

    if method == "OPTIONS" {
        return ReqKind::Options;
    }

    if (method == "GET" || method == "HEAD")
        && header(head, "accept")
            .map(|a| contains_ci(a, "application/nostr+json"))
            .unwrap_or(false)
    {
        return ReqKind::Nip11;
    }

    ReqKind::Other
}

/// Sec-WebSocket-Accept from Sec-WebSocket-Key (RFC 6455), same recipe as
/// nostr-relay-builder's `examples/hyper.rs`.
pub fn derive_accept_key(request_key: &[u8]) -> String {
    const WS_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut engine = Sha1Hash::engine();
    engine.input(request_key);
    engine.input(WS_GUID);
    let hash: Sha1Hash = Sha1Hash::from_engine(engine);
    BASE64_STANDARD.encode(hash.as_byte_array())
}

/// Peek the request head without consuming it, returning the byte length of the
/// head up to and including the terminating CRLFCRLF.
async fn peek_head_len(stream: &TcpStream) -> anyhow::Result<usize> {
    let mut buf = vec![0u8; MAX_HEAD];
    loop {
        let n = stream.peek(&mut buf).await?;
        if n == 0 {
            anyhow::bail!("connection closed before request head");
        }
        if let Some(pos) = find_crlf_crlf(&buf[..n]) {
            return Ok(pos + 4);
        }
        if n >= buf.len() {
            anyhow::bail!("request head exceeds {MAX_HEAD} bytes");
        }
        // Head not fully arrived yet; peek returns the same buffered prefix
        // until more bytes land — yield briefly to avoid a busy spin.
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn find_crlf_crlf(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Handle one accepted connection: read the head, then dispatch.
pub async fn handle_conn(
    relay: LocalRelay,
    nip11_json: &str,
    mut stream: TcpStream,
    peer: SocketAddr,
) -> anyhow::Result<()> {
    // Bound the whole head read so a stalled client can't pin a task forever.
    let head_len = tokio::time::timeout(Duration::from_secs(10), peek_head_len(&stream)).await??;
    let mut head_buf = vec![0u8; head_len];
    stream.read_exact(&mut head_buf).await?;
    let head = String::from_utf8_lossy(&head_buf);

    match classify(&head) {
        ReqKind::WebSocket(key) => {
            let accept = derive_accept_key(key.as_bytes());
            let resp = format!(
                "HTTP/1.1 101 Switching Protocols\r\n\
                 Upgrade: websocket\r\n\
                 Connection: Upgrade\r\n\
                 Sec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            stream.write_all(resp.as_bytes()).await?;
            stream.flush().await?;
            // Stream is now an upgraded (post-101) raw socket, exactly what
            // take_connection -> take_upgraded (from_raw_socket, Role::Server)
            // expects.
            relay
                .take_connection(stream, peer)
                .await
                .map_err(|e| anyhow::anyhow!("relay connection failed: {e}"))?;
        }
        ReqKind::Nip11 => {
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/nostr+json\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Access-Control-Allow-Headers: *\r\n\
                 Access-Control-Allow-Methods: GET, OPTIONS\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                nip11_json.len(),
                nip11_json
            );
            stream.write_all(resp.as_bytes()).await?;
            stream.flush().await?;
        }
        ReqKind::Options => {
            let resp = "HTTP/1.1 204 No Content\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Access-Control-Allow-Methods: GET, OPTIONS\r\n\
                 Access-Control-Allow-Headers: *\r\n\
                 Content-Length: 0\r\n\
                 Connection: close\r\n\r\n";
            stream.write_all(resp.as_bytes()).await?;
            stream.flush().await?;
        }
        ReqKind::Other => {
            let body = "This is a Nostr relay. Connect over WebSocket, or request its \
                        NIP-11 document with `Accept: application/nostr+json`.";
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain; charset=utf-8\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).await?;
            stream.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_websocket_upgrade() {
        let head = "GET / HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\n\
                    Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZQ==\r\n\
                    Sec-WebSocket-Version: 13\r\n\r\n";
        assert_eq!(
            classify(head),
            ReqKind::WebSocket("dGhlIHNhbXBsZQ==".to_string())
        );
    }

    #[test]
    fn detects_nip11() {
        let head = "GET / HTTP/1.1\r\nHost: x\r\nAccept: application/nostr+json\r\n\r\n";
        assert_eq!(classify(head), ReqKind::Nip11);
    }

    #[test]
    fn websocket_wins_over_accept_header() {
        // A client that sends both must be treated as a websocket upgrade.
        let head = "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                    Sec-WebSocket-Key: abc\r\nAccept: application/nostr+json\r\n\r\n";
        assert_eq!(classify(head), ReqKind::WebSocket("abc".to_string()));
    }

    #[test]
    fn plain_get_is_other() {
        let head = "GET / HTTP/1.1\r\nHost: x\r\nAccept: text/html\r\n\r\n";
        assert_eq!(classify(head), ReqKind::Other);
    }

    #[test]
    fn options_is_preflight() {
        let head = "OPTIONS / HTTP/1.1\r\nHost: x\r\nAccess-Control-Request-Method: GET\r\n\r\n";
        assert_eq!(classify(head), ReqKind::Options);
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let head = "GET / HTTP/1.1\r\nACCEPT: Application/Nostr+JSON\r\n\r\n";
        assert_eq!(classify(head), ReqKind::Nip11);
    }

    #[test]
    fn accept_key_matches_rfc6455_example() {
        // RFC 6455 §1.3 canonical example.
        assert_eq!(
            derive_accept_key(b"dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}
