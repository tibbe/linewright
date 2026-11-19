//! What keeps a trial's sessions' `gh` from the task's repository on
//! GitHub (`docs/specs/benchmarks.md`, The repository on GitHub).

use std::convert::Infallible;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use percent_encoding::percent_decode_str;
use serde_json::Value;
use tokio::net::UnixListener;
use tracing::{info, warn};

use crate::domain::Repo;
use crate::harness::GhReach;

/// A configuration of `gh` that a [`Gate`] serves: only a gate makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhConfigDir(PathBuf);

impl GhConfigDir {
    /// The directory, for `GH_CONFIG_DIR`.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Where a [`Gate`] sends the requests it passes.
#[derive(Clone, Debug)]
pub enum Upstream {
    /// `https://` and the request's `Host`.
    GitHub,
    /// This URL, whatever the request's `Host`.
    #[cfg(test)]
    At(reqwest::Url),
}

/// Serves a configuration of `gh` whose requests come to it: it refuses
/// each that names the task's repository, and passes the others to GitHub
/// unchanged, and their answers back unchanged. Dropping it stops serving
/// and removes the configuration.
pub struct Gate {
    config: GhConfigDir,
    serving: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

const CONFIG: &str = "config.yml";
const SOCKET: &str = "gh.sock";

/// Not passed on by a proxy: <https://www.rfc-editor.org/rfc/rfc9110#section-7.6.1>.
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

impl Gate {
    /// Starts serving, for a trial of `repo`, a configuration of `gh` in a
    /// private directory of its own under the temporary directory, the
    /// requests it passes sent to `upstream`.
    ///
    /// # Errors
    ///
    /// When the directory, the configuration or the socket cannot be made,
    /// naming its path; or there is no HTTP client.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    pub fn start(repo: &Repo, upstream: Upstream) -> io::Result<Gate> {
        let dir = tempfile::Builder::new().prefix("lw-gh-").tempdir()?;
        let socket = dir.path().join(SOCKET);
        let listener = UnixListener::bind(&socket).map_err(|e| {
            io::Error::new(e.kind(), format!("listening on {}: {e}", socket.display()))
        })?;
        let config = dir.path().join(CONFIG);
        let yaml = serde_yaml_ng::to_string(&serde_json::json!({ "http_unix_socket": socket }))
            .map_err(io::Error::other)?;
        std::fs::write(&config, yaml)
            .map_err(|e| io::Error::new(e.kind(), format!("writing {}: {e}", config.display())))?;
        let forward = Arc::new(Forward {
            repo: repo.clone(),
            client: upstream.client().map_err(io::Error::other)?,
            upstream,
        });
        Ok(Gate {
            config: GhConfigDir(dir.path().to_path_buf()),
            serving: tokio::spawn(serve(listener, forward)),
            _dir: dir,
        })
    }

    /// How a session's `gh` reaches GitHub through this gate.
    pub fn gh(&self) -> GhReach {
        GhReach::Through(self.config.clone())
    }

    /// The configuration of `gh` this gate serves.
    pub fn config(&self) -> &GhConfigDir {
        &self.config
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

impl Upstream {
    /// An HTTP client that follows no redirect and leaves the bodies as
    /// they come.
    fn client(&self) -> reqwest::Result<reqwest::Client> {
        let builder = crate::store::github::client_builder_trusting_the_host_store_alone()
            .redirect(reqwest::redirect::Policy::none());
        match self {
            Upstream::GitHub => builder.build(),
            #[cfg(test)]
            Upstream::At(_) => builder.no_proxy().build(),
        }
    }

    /// Where a request to `host` goes, its target appended.
    fn base(&self, host: &str) -> String {
        match self {
            Upstream::GitHub => format!("https://{host}"),
            #[cfg(test)]
            Upstream::At(url) => url.as_str().trim_end_matches('/').to_string(),
        }
    }
}

struct Forward {
    repo: Repo,
    upstream: Upstream,
    client: reqwest::Client,
}

type Answer = Response<BoxBody<Bytes, reqwest::Error>>;

/// How long serving waits after failing to accept a connection, so that a
/// lasting failure, such as running out of file descriptors, does not spin.
const AFTER_FAILED_ACCEPT: Duration = Duration::from_millis(100);

/// Serves each connection to `listener` on a task of its own, until it is
/// aborted, which ends them all.
async fn serve(listener: UnixListener, forward: Arc<Forward>) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let forward = forward.clone();
                    connections.spawn(async move {
                        let service = hyper::service::service_fn(move |request| {
                            let forward = forward.clone();
                            async move { Ok::<_, Infallible>(answer(&forward, request).await) }
                        });
                        let served = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                        if let Err(e) = served {
                            warn!(error = %e, "serving a trial's gh failed");
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, "accepting a connection of a trial's gh failed");
                    tokio::time::sleep(AFTER_FAILED_ACCEPT).await;
                }
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

async fn answer(forward: &Forward, request: Request<Incoming>) -> Answer {
    let (parts, body) = request.into_parts();
    let target = parts
        .uri
        .path_and_query()
        .map_or("/", |target| target.as_str());
    let body = match body.collect().await {
        Ok(body) => body.to_bytes(),
        Err(e) => return bad_gateway(&format!("the request could not be read: {e}")),
    };
    let refusal = || {
        info!(
            method = %parts.method,
            target,
            "refused a request of a trial's gh for the task's repository"
        );
        error_answer(
            StatusCode::FORBIDDEN,
            &format!(
                "The repository {} on GitHub is not available in a trial.",
                forward.repo
            ),
        )
    };
    if names_repository(&forward.repo, target, &body) {
        return refusal();
    }
    let Some(host) = parts
        .headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
    else {
        return bad_gateway("the request has no Host");
    };
    let url = format!("{}{target}", forward.upstream.base(host));
    let Ok(url) = reqwest::Url::parse(&url) else {
        return bad_gateway(&format!("{url} is not a URL"));
    };
    if names_repository(&forward.repo, &sent_target(&url), &body) {
        return refusal();
    }
    let mut headers = parts.headers.clone();
    headers.remove(header::HOST);
    drop_hop_by_hop(&mut headers);
    let sent = forward
        .client
        .request(parts.method.clone(), url.clone())
        .headers(headers)
        .body(body)
        .send()
        .await;
    match sent {
        Ok(answered) => {
            let mut answered: Response<reqwest::Body> = answered.into();
            drop_hop_by_hop(answered.headers_mut());
            answered.map(BodyExt::boxed)
        }
        Err(e) => bad_gateway(&format!("the request could not be passed to {url}: {e}")),
    }
}

/// The path and query that a request to `url` sends: parsing it resolved
/// its dot segments, such as `x/../`
/// (<https://url.spec.whatwg.org/#path-state>).
fn sent_target(url: &reqwest::Url) -> String {
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

/// Removes the hop-by-hop headers, and those the `Connection` header names.
fn drop_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::try_from(name.trim()).ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

/// A `502` whose message says `why`.
fn bad_gateway(why: &str) -> Answer {
    error_answer(StatusCode::BAD_GATEWAY, why)
}

/// An answer with `status` whose body is as GitHub's errors are,
/// `{"message": text}`.
fn error_answer(status: StatusCode, text: &str) -> Answer {
    let body = serde_json::json!({ "message": text }).to_string();
    let mut answer = Response::new(
        Full::new(Bytes::from(body))
            .map_err(|never| match never {})
            .boxed(),
    );
    *answer.status_mut() = status;
    answer.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    answer
}

/// Whether a request with `target`, its path and query, and `body` names
/// `repo`: it holds the whole `OWNER/NAME`, or its body holds the owner and
/// the name each as a whole quoted value, whatever their letter case and
/// percent-encoding. When the owner and the name are the same, the body
/// holds two such values.
pub fn names_repository(repo: &Repo, target: &str, body: &[u8]) -> bool {
    let owner = repo.owner().to_lowercase();
    let name = repo.name().to_lowercase();
    let mut values = Vec::new();
    if let Ok(json) = serde_json::from_slice::<Value>(body) {
        strings(&json, &mut values);
    }
    let values: Vec<String> = values.iter().map(|value| normalized(value)).collect();
    let mut body_texts = vec![normalized(&String::from_utf8_lossy(body))];
    body_texts.extend(values.iter().cloned());
    let whole = std::iter::once(normalized(target))
        .chain(body_texts.iter().cloned())
        .any(|text| holds_whole(&text, &owner, &name));
    let quoted = |part: &str| {
        let in_quotes = format!("\"{part}\"");
        let as_values = values.iter().filter(|value| *value == part).count();
        let in_texts = body_texts
            .iter()
            .map(|text| text.matches(&in_quotes).count())
            .max()
            .unwrap_or(0);
        as_values.max(in_texts)
    };
    let both_quoted = if owner == name {
        quoted(&owner) >= 2
    } else {
        quoted(&owner) >= 1 && quoted(&name) >= 1
    };
    whole || both_quoted
}

/// `text` percent-decoded and lowercased.
fn normalized(text: &str) -> String {
    percent_decode_str(text).decode_utf8_lossy().to_lowercase()
}

/// Adds to `found` each string in `json`, its keys among them.
fn strings(json: &Value, found: &mut Vec<String>) {
    match json {
        Value::String(text) => found.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                strings(item, found);
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                found.push(key.clone());
                strings(value, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// Whether `text` holds `owner/name` with no letter, digit or `-` before
/// it, and no letter, digit, `.`, `_` or `-` after it.
fn holds_whole(text: &str, owner: &str, name: &str) -> bool {
    let whole = format!("{owner}/{name}");
    let in_login = |c: char| c.is_ascii_alphanumeric() || c == '-';
    let in_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_');
    text.char_indices().any(|(at, _)| {
        text[at..].starts_with(&whole)
            && !text[..at].chars().next_back().is_some_and(in_login)
            && !text[at + whole.len()..].chars().next().is_some_and(in_name)
    })
}

#[cfg(test)]
mod tests {
    use std::panic::Location;
    use std::sync::Mutex;

    use hyper::Method;

    use super::*;

    const REPO: &str = "Acme/Widgets";

    /// The fixture `name` in `dir`, split into its head's lines and its
    /// body.
    #[track_caller]
    fn fixture(dir: &str, name: &str) -> (Vec<String>, Bytes) {
        let path = format!(
            "{}/src/trial/fixtures/{dir}/{name}.http",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        let end = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or_else(|| panic!("{path} has no end of its head"));
        let head = String::from_utf8(raw[..end].to_vec()).unwrap();
        let lines = head.split("\r\n").map(String::from).collect();
        (lines, Bytes::copy_from_slice(&raw[end + 4..]))
    }

    /// The request `gh` sent, recorded in the fixture `name`.
    #[track_caller]
    fn recorded(name: &str) -> Request<Bytes> {
        let (lines, body) = fixture("gh-requests", name);
        let mut first = lines[0].split(' ');
        let mut request = Request::builder()
            .method(first.next().unwrap())
            .uri(first.next().unwrap());
        for line in &lines[1..] {
            let (name, value) = line
                .split_once(": ")
                .unwrap_or_else(|| panic!("{line:?} in {name} is not a header"));
            request = request.header(name, value);
        }
        request.body(body).unwrap()
    }

    /// The answer GitHub gave, recorded in the fixture `name`.
    #[track_caller]
    fn answered(name: &str) -> Response<Bytes> {
        let (lines, body) = fixture("github-answers", name);
        let status = lines[0].split(' ').nth(1).unwrap();
        let mut answer = Response::builder().status(status);
        for line in &lines[1..] {
            let (name, value) = line
                .split_once(": ")
                .unwrap_or_else(|| panic!("{line:?} in {name} is not a header"));
            answer = answer.header(name, value);
        }
        answer.body(body).unwrap()
    }

    #[track_caller]
    fn names(target: &str, body: &[u8]) -> bool {
        names_repository(&REPO.parse().unwrap(), target, body)
    }

    #[track_caller]
    fn names_recorded(name: &str) -> bool {
        let request = recorded(name);
        names(&request.uri().to_string(), request.body())
    }

    #[test_log::test]
    fn the_requests_gh_sends_for_the_repository_name_it() {
        for name in [
            "api-repos",
            "api-percent-encoded",
            "api-upper-case",
            "repo-view",
            "issue-view",
            "pr-list",
            "release-list",
            "search-issues",
            "graphql-inline",
            "graphql-variables",
        ] {
            assert!(names_recorded(name), "{name}");
        }
    }

    #[test_log::test]
    fn the_requests_gh_sends_for_other_repositories_or_none_do_not_name_it() {
        for name in [
            "api-longer-name",
            "api-pages-name",
            "api-other-owner",
            "graphql-owner-alone",
            "graphql-introspection",
        ] {
            assert!(!names_recorded(name), "{name}");
        }
    }

    #[test_log::test]
    fn a_body_names_it_with_both_quoted_values_apart_and_not_with_one() {
        assert!(names(
            "/graphql",
            br#"{"a": ["Widgets"], "b": {"c": "ACME"}}"#
        ));
        assert!(names("/upload", br#"owner="acme" then name="widgets""#));
        assert!(!names("/graphql", br#"{"name": "Widgets"}"#));
        assert!(!names(
            "/graphql",
            br#"{"owner": "Acme", "name": "Widgets-docs"}"#
        ));
        assert!(!names("/graphql", br"owner=acme name=widgets"));
    }

    #[test_log::test]
    fn when_the_owner_is_the_name_a_body_names_it_with_two_quoted_values_and_not_with_one() {
        let profile = "acme/acme".parse().unwrap();

        assert!(names_repository(
            &profile,
            "/graphql",
            br#"{"variables": {"owner": "acme", "repo": "Acme"}}"#
        ));
        assert!(names_repository(
            &profile,
            "/graphql",
            br#"{"query": "repository(owner:\"acme\", name:\"acme\")"}"#
        ));
        assert!(!names_repository(
            &profile,
            "/graphql",
            br#"{"variables": {"owner": "acme", "repo": "other"}}"#
        ));
    }

    #[test_log::test]
    fn a_whole_owner_and_name_names_it_wherever_they_are_and_however_escaped() {
        assert!(names("/search?q=repo%3Aacme%2Fwidgets", b""));
        assert!(names("/graphql", br#"{"query": "acme\/widgets"}"#));
        assert!(names("/graphql", br#"{"query": "acme\u002fwidgets"}"#));
        assert!(names("/repos/Acme/Widgets", b""));
        assert!(!names("/repos/xacme/widgets", b""));
        assert!(!names("/repos/acme/widgets_2", b""));
        assert!(!names("/repos/acme-x/widgets", b""));
    }

    #[test_log::test]
    fn a_whole_owner_and_name_names_it_after_a_longer_one_it_overlaps() {
        let profile = "acme/acme".parse().unwrap();

        assert!(names_repository(&profile, "/q=xacme/acme/acme", b""));
    }

    /// A request as the fake upstream saw it.
    #[derive(Debug, PartialEq, Eq)]
    struct Seen {
        method: Method,
        target: String,
        headers: HeaderMap,
        body: Bytes,
    }

    /// A local server that answers every request with `answer`: its URL,
    /// and the requests it was sent.
    #[track_caller]
    fn fake_upstream(
        answer: Response<Bytes>,
    ) -> impl Future<Output = (reqwest::Url, Arc<Mutex<Vec<Seen>>>)> {
        let caller = Location::caller();
        async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap_or_else(|e| panic!("listening at {caller}: {e}"));
            let url = format!("http://{}/", listener.local_addr().unwrap())
                .parse()
                .unwrap();
            let seen = Arc::new(Mutex::new(vec![]));
            let kept = seen.clone();
            let (answer_head, answer_body) = answer.into_parts();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let kept = kept.clone();
                    let (answer_head, answer_body) = (answer_head.clone(), answer_body.clone());
                    tokio::spawn(async move {
                        let service =
                            hyper::service::service_fn(move |request: Request<Incoming>| {
                                let kept = kept.clone();
                                let reply = Response::from_parts(
                                    answer_head.clone(),
                                    Full::new(answer_body.clone()),
                                );
                                async move {
                                    let (asked, asked_body) = request.into_parts();
                                    let was = Seen {
                                        method: asked.method,
                                        target: asked.uri.to_string(),
                                        headers: asked.headers,
                                        body: asked_body.collect().await.unwrap().to_bytes(),
                                    };
                                    kept.lock().unwrap().push(was);
                                    Ok::<_, Infallible>(reply)
                                }
                            });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service)
                            .await;
                    });
                }
            });
            (url, seen)
        }
    }

    /// What `gate` answers `request`.
    #[track_caller]
    fn ask(
        gate: &Gate,
        request: Request<Bytes>,
    ) -> impl Future<Output = (StatusCode, HeaderMap, Bytes)> {
        let caller = Location::caller();
        let yaml = std::fs::read_to_string(gate.config().path().join("config.yml"))
            .unwrap_or_else(|e| panic!("reading the configuration at {caller}: {e}"));
        let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&yaml).unwrap();
        let socket = PathBuf::from(yaml["http_unix_socket"].as_str().unwrap());
        async move {
            let stream = tokio::net::UnixStream::connect(&socket)
                .await
                .unwrap_or_else(|e| panic!("connecting at {caller}: {e}"));
            let (mut sender, connection) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream))
                    .await
                    .unwrap();
            tokio::spawn(connection);
            let answer = sender
                .send_request(request.map(Full::new))
                .await
                .unwrap_or_else(|e| panic!("asking at {caller}: {e}"));
            let (parts, body) = answer.into_parts();
            let body = body.collect().await.unwrap().to_bytes();
            (parts.status, parts.headers, body)
        }
    }

    #[track_caller]
    fn gate_to(url: reqwest::Url) -> Gate {
        Gate::start(&REPO.parse().unwrap(), Upstream::At(url)).unwrap()
    }

    #[test_log::test(tokio::test)]
    async fn a_request_naming_the_repository_is_refused_with_a_message_gh_shows() {
        let (url, seen) = fake_upstream(answered("repo-gzip")).await;
        let gate = gate_to(url);

        let (status, headers, body) = ask(&gate, recorded("api-upper-case")).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(headers["content-type"], "application/json");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "message": "The repository Acme/Widgets on GitHub is not available in a trial."
            })
        );
        assert_eq!(*seen.lock().unwrap(), vec![]);
    }

    #[test_log::test(tokio::test)]
    async fn a_request_whose_dot_segments_lead_to_the_repository_is_refused() {
        let (url, seen) = fake_upstream(answered("repo-gzip")).await;
        let gate = gate_to(url);
        let mut request = recorded("api-other-owner");
        *request.uri_mut() = "/repos/acme/x/%2E%2e/widgets".parse().unwrap();

        let (status, _, _) = ask(&gate, request).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(*seen.lock().unwrap(), vec![]);
    }

    #[test_log::test(tokio::test)]
    async fn another_request_is_passed_as_gh_sent_it_and_its_answer_back_unchanged() {
        let (url, seen) = fake_upstream(answered("tarball-redirect")).await;
        let gate = gate_to(url);
        let mut request = recorded("graphql-owner-alone");
        *request.uri_mut() = "/graphql?a=1".parse().unwrap();
        let sent = request.headers().clone();
        let body = request.body().clone();
        request
            .headers_mut()
            .insert("connection", HeaderValue::from_static("close, x-trace"));
        request
            .headers_mut()
            .insert("x-trace", HeaderValue::from_static("on"));

        let (status, headers, answer) = ask(&gate, request).await;

        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(
            headers["location"],
            "https://codeload.github.com/cli/cli/legacy.tar.gz/refs/tags/v2.0.0"
        );
        assert_eq!(headers["x-ratelimit-resource"], "core");
        assert_eq!(answer, Bytes::new());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].method, Method::POST);
        assert_eq!(seen[0].target, "/graphql?a=1");
        assert_eq!(seen[0].body, body);
        for (name, value) in &sent {
            if name != "host" && name != "connection" {
                assert_eq!(seen[0].headers.get(name), Some(value), "{name}");
            }
        }
        assert_eq!(seen[0].headers.get("x-trace"), None);
    }

    #[test_log::test(tokio::test)]
    async fn a_zipped_answer_comes_back_as_it_was() {
        let recorded_answer = answered("repo-gzip");
        let zipped = recorded_answer.body().clone();
        let (url, _) = fake_upstream(recorded_answer).await;
        let gate = gate_to(url);

        let (status, headers, body) = ask(&gate, recorded("api-other-owner")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["content-encoding"], "gzip");
        assert_eq!(&body[..2], [0x1f, 0x8b]);
        assert_eq!(body, zipped);
    }

    #[test_log::test(tokio::test)]
    async fn a_dropped_gate_removes_its_configuration() {
        let gate = Gate::start(&REPO.parse().unwrap(), Upstream::GitHub).unwrap();
        let config = gate.config().clone();

        drop(gate);

        assert!(!config.path().exists());
    }
}
