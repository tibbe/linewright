//! `serve-fake`, through the system: what an agent serves the web UI with to
//! drive it in a browser, over the seed's fake GitHub and fake agent.

use std::panic::Location;
use std::process::Stdio;
use std::time::Duration;

use scraper::{Html, Selector};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// A running `serve-fake`, the address it said it serves the web UI on, and
/// the command it said gives the browser its time.
struct Served {
    child: Child,
    url: String,
    browser_clock: String,
}

impl Served {
    /// Starts `serve-fake`, failing unless it says its two lines within a
    /// minute.
    #[track_caller]
    fn start() -> impl Future<Output = Served> {
        let caller = Location::caller();
        async move {
            let mut child = Command::new(env!("CARGO_BIN_EXE_serve-fake"))
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap_or_else(|e| panic!("starting serve-fake at {caller}: {e}"));
            let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
            let said = tokio::time::timeout(Duration::from_secs(60), async {
                let url = lines.next_line().await.ok().flatten();
                let browser_clock = lines.next_line().await.ok().flatten();
                url.zip(browser_clock)
            })
            .await;
            let (url, browser_clock) = said
                .unwrap_or_else(|_| panic!("serve-fake said nothing in time at {caller}"))
                .unwrap_or_else(|| panic!("serve-fake said too little at {caller}"));
            Served {
                child,
                url,
                browser_clock,
            }
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.url.trim_end_matches('/'))
    }

    /// The page at `path`, failing unless it is there.
    #[track_caller]
    fn page(&self, path: &str) -> impl Future<Output = Html> + '_ {
        let caller = Location::caller();
        let url = self.url(path);
        async move {
            let response = reqwest::get(&url)
                .await
                .unwrap_or_else(|e| panic!("reading {url} at {caller}: {e}"));
            assert_eq!(response.status(), 200, "{url} at {caller}");
            let text = response
                .text()
                .await
                .unwrap_or_else(|e| panic!("reading {url} at {caller}: {e}"));
            Html::parse_document(&text)
        }
    }

    /// The page at `path` once `shows` holds of it, failing after a minute.
    #[track_caller]
    fn eventually_page<'a>(
        &'a self,
        path: &str,
        shows: impl Fn(&Html) -> bool + 'a,
    ) -> impl Future<Output = Html> + 'a {
        let caller = Location::caller();
        let path = path.to_string();
        async move {
            let waited = tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    let html = self.page(&path).await;
                    if shows(&html) {
                        return html;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            })
            .await;
            waited.unwrap_or_else(|_| panic!("{path} never showed it at {caller}"))
        }
    }

    /// The first event with an ID the stream at `path` sends, failing after
    /// a minute.
    #[track_caller]
    fn first_event(&self, path: &str) -> impl Future<Output = String> + '_ {
        let caller = Location::caller();
        let url = self.url(path);
        async move {
            let mut response = reqwest::get(&url)
                .await
                .unwrap_or_else(|e| panic!("following {url} at {caller}: {e}"));
            assert_eq!(response.status(), 200, "{url} at {caller}");
            let mut unread = String::new();
            let waited = tokio::time::timeout(Duration::from_secs(60), async {
                while let Ok(Some(chunk)) = response.chunk().await {
                    unread.push_str(&String::from_utf8_lossy(&chunk));
                    while let Some(end) = unread.find("\n\n") {
                        let block: String = unread.drain(..end + 2).collect();
                        if block.lines().any(|line| line.starts_with("id: ")) {
                            return Some(block);
                        }
                    }
                }
                None
            })
            .await;
            waited
                .unwrap_or_else(|_| panic!("{url} sent nothing in time at {caller}"))
                .unwrap_or_else(|| panic!("{url} ended at {caller}"))
        }
    }

    /// Stops `serve-fake` as `SIGQUIT` does, failing unless it exits 0
    /// within 30 seconds.
    #[track_caller]
    fn stop(mut self) -> impl Future<Output = ()> {
        let caller = Location::caller();
        async move {
            let pid = self.child.id().expect("serve-fake is running");
            rustix::process::kill_process(
                rustix::process::Pid::from_raw(pid.try_into().expect("a PID")).expect("a PID"),
                rustix::process::Signal::QUIT,
            )
            .unwrap_or_else(|e| panic!("stopping serve-fake at {caller}: {e}"));
            let status = tokio::time::timeout(Duration::from_secs(30), self.child.wait())
                .await
                .unwrap_or_else(|_| panic!("serve-fake did not stop in time at {caller}"))
                .unwrap_or_else(|e| panic!("waiting for serve-fake at {caller}: {e}"));
            assert!(status.success(), "{status} at {caller}");
        }
    }
}

#[track_caller]
fn attributes(html: &Html, selector: &str, attribute: &str) -> Vec<String> {
    html.select(&Selector::parse(selector).unwrap())
        .filter_map(|element| element.attr(attribute))
        .map(str::to_string)
        .collect()
}

#[track_caller]
fn text(html: &Html) -> String {
    html.select(&Selector::parse("body").unwrap())
        .flat_map(|body| body.text())
        .collect()
}

/// The links `html` has to a run.
#[track_caller]
fn runs(html: &Html) -> Vec<String> {
    attributes(html, "a[href^='/runs/']", "href")
}

/// Where the page follows its run from, while the run is in flight.
#[track_caller]
fn followed_from(html: &Html) -> Option<String> {
    attributes(html, "body[hx-sse\\:connect]", "hx-sse:connect").pop()
}

/// The link `html` has to an implement run.
#[track_caller]
fn implement_run(html: &Html) -> Option<String> {
    runs(html).into_iter().find(|run| run.contains("implement"))
}

/// When each duration `html` shows still running started, which the
/// browser counts from.
#[track_caller]
fn running_since(html: &Html) -> Vec<String> {
    attributes(html, "time[data-since]", "data-since")
}

/// Each time `html` shows: the instants its `time` elements say, leaving out
/// the settled durations, which ISO 8601 starts with `P`, and when each
/// running duration started.
#[track_caller]
fn times(html: &Html) -> Vec<String> {
    let mut times: Vec<String> = attributes(html, "time[datetime]", "datetime")
        .into_iter()
        .filter(|time| !time.starts_with('P'))
        .collect();
    times.extend(running_since(html));
    times
}

#[test_log::test(tokio::test)]
async fn serve_fake_serves_the_seeds_board_and_its_runs_in_flight_go_on_live() {
    let served = Served::start().await;

    served
        .eventually_page("/", |html| {
            let board = text(html);
            board.contains("Show a session's duration in its heading")
                && board.contains("The board's lanes don't fit a phone's screen")
        })
        .await;

    let listed = served
        .eventually_page("/runs", |html| implement_run(html).is_some())
        .await;
    let run = implement_run(&listed).expect("an implement run");
    let page = served
        .eventually_page(&run, |html| followed_from(html).is_some())
        .await;
    let event = served
        .first_event(&followed_from(&page).expect("followed"))
        .await;
    assert!(event.contains("entry"), "{event}");

    served.stop().await;
}

#[test_log::test(tokio::test)]
async fn serve_fake_and_the_browser_clock_it_names_tell_the_time_from_the_seeds_start() {
    let served = Served::start().await;

    let listed = served
        .eventually_page("/runs", |html| implement_run(html).is_some())
        .await;
    let run = implement_run(&listed).expect("an implement run");
    let page = served
        .eventually_page(&run, |html| !running_since(html).is_empty())
        .await;

    let times = times(&page);
    assert!(
        times.iter().all(|time| time.starts_with("2026-03-02T09:0")),
        "{times:?}"
    );
    let shift: i64 = served
        .browser_clock
        .strip_prefix(
            "playwright-cli run-code \"async page => { await page.clock.setSystemTime(Date.now() + (",
        )
        .and_then(|rest| rest.strip_suffix(")); }\""))
        .unwrap_or_else(|| panic!("{}", served.browser_clock))
        .parse()
        .unwrap();
    let browser = chrono::Utc::now() + chrono::TimeDelta::milliseconds(shift);
    assert!(
        browser.to_rfc3339().starts_with("2026-03-02T09:0"),
        "{browser}"
    );

    served.stop().await;
}
