mod server;

use herdr_matrix::Error;
use herdr_matrix::matrix::{Http, Matrix as _};
use serde_json::json;
use server::Server;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const T: Duration = Duration::from_secs(30);

fn server(file: Arc<Mutex<Vec<u8>>>) -> Server {
    Server::new(move |path| {
        if path.contains("/media/download/") {
            (200, "image/png".into(), file.lock().expect("lock").clone())
        } else if path.starts_with("/_matrix/media/v3/upload") {
            (
                200,
                "application/json".into(),
                br#"{"content_uri": "mxc://s/u"}"#.to_vec(),
            )
        } else {
            (200, "application/json".into(), br#"{"ok": true}"#.to_vec())
        }
    })
}

#[test]
fn client_api() {
    let server = server(Arc::default());
    let mut matrix = Http::new(&format!("{}/", server.url), "tok");
    let body = json!({"a": 1});
    assert_eq!(
        matrix
            .call(
                "PUT",
                "/rooms/r/send/x/1",
                Some(&body),
                &[("since", "s 1")],
                T
            )
            .expect("call"),
        json!({"ok": true})
    );
    let got = server.last();
    assert_eq!(
        (
            got.method.as_str(),
            got.path.as_str(),
            serde_json::from_slice::<serde_json::Value>(&got.body).expect("JSON")
        ),
        (
            "PUT",
            "/_matrix/client/v3/rooms/r/send/x/1?since=s+1",
            json!({"a": 1})
        )
    );
    assert_eq!(got.headers["authorization"], "Bearer tok");
    assert_ne!(matrix.next_txn(), matrix.next_txn());
    matrix.call("GET", "/sync", None, &[], T).expect("GET");
    assert_eq!(server.last().method, "GET");
}

#[test]
fn upload() {
    let server = server(Arc::default());
    let mut matrix = Http::new(&server.url, "tok");
    assert_eq!(
        matrix.upload(b"png", "image/png").expect("upload"),
        "mxc://s/u"
    );
    let got = server.last();
    assert_eq!(
        (got.method.as_str(), got.path.as_str(), got.body.as_slice()),
        ("POST", "/_matrix/media/v3/upload", &b"png"[..])
    );
    assert_eq!(got.headers["content-type"], "image/png");
    assert_eq!(got.headers["authorization"], "Bearer tok");
}

#[test]
fn download() {
    let server = server(Arc::new(Mutex::new(b"picture".to_vec())));
    let mut matrix = Http::new(&server.url, "tok");
    assert_eq!(
        matrix.download("mxc://ex.org/a/b").expect("download"),
        b"picture"
    );
    let got = server.last();
    assert_eq!(got.path, "/_matrix/client/v1/media/download/ex.org/a%2Fb");
    assert_eq!(got.headers["authorization"], "Bearer tok");
}

#[test]
fn download_is_capped() {
    let file = Arc::new(Mutex::new(b"picture".to_vec()));
    let server = server(file.clone());
    let mut matrix = Http::new(&server.url, "tok");
    matrix.file_max = 7;
    assert_eq!(matrix.download("mxc://s/m").expect("download"), b"picture");
    file.lock().expect("lock").push(b'!');
    let err = matrix.download("mxc://s/m").expect_err("too large");
    assert!(matches!(err, Error::Refused(_)));
}

#[test]
fn http_error_carries_its_matrix_code() {
    let server = Server::new(|_| {
        (
            400,
            "application/json".into(),
            br#"{"errcode": "M_USER_IN_USE"}"#.to_vec(),
        )
    });
    let mut matrix = Http::new(&server.url, "tok");
    let err = matrix
        .call("POST", "/register", Some(&json!({})), &[], T)
        .expect_err("400");
    assert_eq!(err.errcode().as_deref(), Some("M_USER_IN_USE"));
    assert_eq!(err.to_string(), "the homeserver answered 400 M_USER_IN_USE");
}

#[test]
fn rate_limit_is_waited_out_once() {
    let answers = Arc::new(Mutex::new(vec![
        (
            429,
            br#"{"errcode": "M_LIMIT_EXCEEDED", "retry_after_ms": 50}"#.to_vec(),
        ),
        (200, br#"{"ok": true}"#.to_vec()),
    ]));
    let server = Server::new(move |_| {
        let (code, body) = answers.lock().expect("lock").remove(0);
        (code, "application/json".into(), body)
    });
    let mut matrix = Http::new(&server.url, "tok");
    let started = std::time::Instant::now();
    assert_eq!(
        matrix.call("GET", "/sync", None, &[], T).expect("call"),
        json!({"ok": true})
    );
    assert!(started.elapsed() >= Duration::from_millis(50));
    assert_eq!(server.requests.lock().expect("lock").len(), 2);
}

fn answering(code: u16, kind: &str, body: &[u8]) -> Server {
    let (kind, body) = (kind.to_owned(), body.to_vec());
    Server::new(move |_| (code, kind.clone(), body.clone()))
}

#[test]
fn closed_port_is_a_transport_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address");
    let mut matrix = Http::new(&format!("http://{port}"), "tok");
    let err = matrix
        .call("GET", "/sync", None, &[], T)
        .expect_err("refused");
    assert!(matches!(err, Error::Transport(_)), "{err:?}");
}

#[test]
fn ok_with_broken_json_is_malformed() {
    let server = answering(200, "application/json", b"{nope");
    let mut matrix = Http::new(&server.url, "tok");
    let err = matrix
        .call("GET", "/sync", None, &[], T)
        .expect_err("broken");
    assert!(matches!(err, Error::Malformed(_)), "{err:?}");
}

#[test]
fn server_error_page_has_no_errcode() {
    let server = answering(502, "text/html", b"<html>Bad Gateway</html>");
    let mut matrix = Http::new(&server.url, "tok");
    let err = matrix.call("GET", "/sync", None, &[], T).expect_err("502");
    assert!(matches!(err, Error::Http { status: 502, .. }), "{err:?}");
    assert_eq!(err.errcode(), None);
    assert_eq!(err.to_string(), "the homeserver answered 502 Bad Gateway");
}

#[test]
fn revoked_token_says_what_to_check() {
    let server = answering(
        401,
        "application/json",
        br#"{"errcode": "M_UNKNOWN_TOKEN"}"#,
    );
    let mut matrix = Http::new(&server.url, "tok");
    let err = matrix.call("GET", "/sync", None, &[], T).expect_err("401");
    assert_eq!(
        err.to_string(),
        "the homeserver answered 401 M_UNKNOWN_TOKEN"
    );
    assert!(err.hint().expect("a hint").contains("HERDR_MATRIX_TOKEN"));
}

#[test]
fn upload_without_a_uri_is_malformed() {
    let server = answering(200, "application/json", b"{}");
    let mut matrix = Http::new(&server.url, "tok");
    let err = matrix.upload(b"png", "image/png").expect_err("no uri");
    assert!(matches!(err, Error::Malformed(_)), "{err:?}");
}

#[test]
fn missing_download_is_an_http_error() {
    let server = answering(404, "application/json", br#"{"errcode": "M_NOT_FOUND"}"#);
    let mut matrix = Http::new(&server.url, "tok");
    let err = matrix.download("mxc://s/m").expect_err("404");
    assert!(matches!(err, Error::Http { status: 404, .. }), "{err:?}");
}

#[test]
fn odd_mxc_paths_are_quoted() {
    let server = server(Arc::new(Mutex::new(b"x".to_vec())));
    let mut matrix = Http::new(&server.url, "tok");
    matrix.download("mxc://ex.org:8448/id").expect("download");
    assert_eq!(
        server.last().path,
        "/_matrix/client/v1/media/download/ex.org%3A8448/id"
    );
    matrix.download("bare").expect("download");
    assert_eq!(
        server.last().path,
        "/_matrix/client/v1/media/download/bare/"
    );
}

#[test]
fn slow_server_times_out() {
    let server = Server::slow(Duration::from_secs(2), |_| {
        (200, "application/json".into(), b"{}".to_vec())
    });
    let mut matrix = Http::new(&server.url, "tok");
    let started = std::time::Instant::now();
    let err = matrix
        .call("GET", "/sync", None, &[], Duration::from_millis(200))
        .expect_err("timed out");
    assert!(matches!(err, Error::Transport(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(1));
}
