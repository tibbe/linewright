//! `serve-fake`, through the system: what an agent serves the web UI with to
//! drive it in a browser, over the seed's fake GitHub and fake agent.

use std::process::Stdio;
use std::time::Duration;

use scraper::{Html, Selector};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// A running `serve-fake`, the address it said it serves the web UI on, the
/// command it said gives the browser its time, and the `Cookie` of a
/// browser it signed in.
struct Served {
    child: Child,
    url: String,
    browser_clock: String,
    cookie: String,
}

impl Served {
    /// Starts `serve-fake`, failing unless it says its two lines within a
    /// minute.
    async fn start() -> Served {
        let mut child = Command::new(env!("CARGO_BIN_EXE_serve-fake"))
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|e| panic!("starting serve-fake: {e}"));
        let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
        let said = tokio::time::timeout(Duration::from_secs(60), async {
            let url = lines.next_line().await.ok().flatten();
            let browser_clock = lines.next_line().await.ok().flatten();
            url.zip(browser_clock)
        })
        .await;
        let (url, browser_clock) = said
            .unwrap_or_else(|_| panic!("serve-fake said nothing in time"))
            .unwrap_or_else(|| panic!("serve-fake said too little"));
        let cookie = signed_in(&url)
            .await
            .unwrap_or_else(|e| panic!("signing in to serve-fake: {e:#}"));
        Served {
            child,
            url,
            browser_clock,
            cookie,
        }
    }

    /// A GET of `url` from the signed-in browser.
    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        browser()
            .build()
            .expect("a browser")
            .get(url)
            .header("Cookie", &self.cookie)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.url.trim_end_matches('/'))
    }

    /// The page at `path`, failing unless it is there.
    async fn page(&self, path: &str) -> Html {
        let url = self.url(path);
        let response = self
            .get(&url)
            .send()
            .await
            .unwrap_or_else(|e| panic!("reading {url}: {e}"));
        assert_eq!(response.status(), 200, "{url}");
        let text = response
            .text()
            .await
            .unwrap_or_else(|e| panic!("reading {url}: {e}"));
        Html::parse_document(&text)
    }

    /// The page at `path` once `shows` holds of it, failing after a minute.
    async fn eventually_page(&self, path: &str, shows: impl Fn(&Html) -> bool) -> Html {
        let waited = tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let html = self.page(path).await;
                if shows(&html) {
                    return html;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
        .await;
        waited.unwrap_or_else(|_| panic!("{path} never showed it"))
    }

    /// The first event with an ID the stream at `path` sends that `holds`
    /// holds of, failing after a minute.
    async fn first_event(&self, path: &str, holds: impl Fn(&str) -> bool) -> String {
        let url = self.url(path);
        let mut response = self
            .get(&url)
            .send()
            .await
            .unwrap_or_else(|e| panic!("following {url}: {e}"));
        assert_eq!(response.status(), 200, "{url}");
        let mut unread = String::new();
        let waited = tokio::time::timeout(Duration::from_secs(60), async {
            while let Ok(Some(chunk)) = response.chunk().await {
                unread.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = unread.find("\n\n") {
                    let block: String = unread.drain(..end + 2).collect();
                    if block.lines().any(|line| line.starts_with("id: ")) && holds(&block) {
                        return Some(block);
                    }
                }
            }
            None
        })
        .await;
        waited
            .unwrap_or_else(|_| panic!("{url} sent nothing in time"))
            .unwrap_or_else(|| panic!("{url} ended"))
    }

    /// Stops `serve-fake` as `SIGQUIT` does, failing unless it exits 0
    /// within 30 seconds.
    async fn stop(mut self) {
        let pid = self.child.id().expect("serve-fake is running");
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(pid.try_into().expect("a PID")).expect("a PID"),
            rustix::process::Signal::QUIT,
        )
        .unwrap_or_else(|e| panic!("stopping serve-fake: {e}"));
        let status = tokio::time::timeout(Duration::from_secs(30), self.child.wait())
            .await
            .unwrap_or_else(|_| panic!("serve-fake did not stop in time"))
            .unwrap_or_else(|e| panic!("waiting for serve-fake: {e}"));
        assert!(status.success(), "{status}");
    }
}

/// A browser as the `verify-web-ui` skill opens it: one that takes
/// `serve-fake`'s self-signed certificate.
fn browser() -> reqwest::ClientBuilder {
    reqwest::Client::builder().tls_danger_accept_invalid_certs(true)
}

/// Signs a browser in to the web UI at `url` as the first account of
/// `serve-fake`'s fake Google, as a browser does, following each redirect
/// by hand: the `Cookie` it then sends.
async fn signed_in(url: &str) -> anyhow::Result<String> {
    use anyhow::Context as _;
    let client = browser()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let location = |response: &reqwest::Response| -> anyhow::Result<String> {
        Ok(response
            .headers()
            .get("location")
            .context("no Location")?
            .to_str()?
            .to_string())
    };
    let cookie = |response: &reqwest::Response| -> anyhow::Result<String> {
        let set = response
            .headers()
            .get("set-cookie")
            .context("no Set-Cookie")?
            .to_str()?;
        Ok(set.split(';').next().unwrap_or_default().to_string())
    };
    let origin = url.trim_end_matches('/');
    let begun = client
        .post(format!("{origin}/auth/google"))
        .header("Origin", origin)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("return=%2F")
        .send()
        .await?;
    let sign_in = cookie(&begun)?;
    let authorize = reqwest::Url::parse(&location(&begun)?)?;
    let choosing = Html::parse_document(&client.get(authorize.clone()).send().await?.text().await?);
    let first = attributes(&choosing, "li a", "href")
        .into_iter()
        .next()
        .context("the fake Google offers no account")?;
    let chosen = client.get(authorize.join(&first)?).send().await?;
    let back = client
        .get(location(&chosen)?)
        .header("Cookie", sign_in)
        .send()
        .await?;
    anyhow::ensure!(back.status() == 303, "signing in: {}", back.status());
    cookie(&back)
}

fn attributes(html: &Html, selector: &str, attribute: &str) -> Vec<String> {
    html.select(&Selector::parse(selector).unwrap())
        .filter_map(|element| element.attr(attribute))
        .map(str::to_string)
        .collect()
}

fn text(html: &Html) -> String {
    html.select(&Selector::parse("body").unwrap())
        .flat_map(|body| body.text())
        .collect()
}

/// The links `html` has to a run.
fn runs(html: &Html) -> Vec<String> {
    attributes(html, "a[href^='/runs/']", "href")
}

/// Where the page follows its run from, while the run is in flight.
fn followed_from(html: &Html) -> Option<String> {
    attributes(html, "#stream[hx-sse\\:connect]", "hx-sse:connect").pop()
}

/// The stream at `followed`, a page's, following its run from the run's
/// first line rather than after the last line the page showed. Panics
/// unless `followed` is a path with a `follow` parameter.
#[track_caller]
fn from_the_start(followed: &str) -> String {
    let mut url = reqwest::Url::parse("https://serve-fake/")
        .and_then(|base| base.join(followed))
        .unwrap_or_else(|e| panic!("{followed}: {e}"));
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(name, value)| {
            let value = if name == "follow" {
                "start".into()
            } else {
                value
            };
            (name.into_owned(), value.into_owned())
        })
        .collect();
    assert!(pairs.iter().any(|(name, _)| name == "follow"), "{followed}");
    url.query_pairs_mut().clear().extend_pairs(pairs);
    format!("{}?{}", url.path(), url.query().unwrap_or_default())
}

/// The link `html` has to an implement run.
fn implement_run(html: &Html) -> Option<String> {
    runs(html).into_iter().find(|run| run.contains("implement"))
}

/// When each duration `html` shows still running started, which the
/// browser counts from.
fn running_since(html: &Html) -> Vec<String> {
    attributes(html, "time[data-since]", "data-since")
}

/// Each time `html` shows: the instants its `time` elements say, leaving out
/// the settled durations, which ISO 8601 starts with `P`, and when each
/// running duration started.
fn times(html: &Html) -> Vec<String> {
    let mut times: Vec<String> = attributes(html, "time[datetime]", "datetime")
        .into_iter()
        .filter(|time| !time.starts_with('P'))
        .collect();
    times.extend(running_since(html));
    times
}

#[test_log::test(tokio::test)]
async fn serve_fake_serves_the_seeds_board_and_streams_its_runs_in_flight_over_https() {
    let served = Served::start().await;
    assert!(
        served.url.starts_with("https://127.0.0.1:") && served.url.ends_with('/'),
        "{}",
        served.url
    );

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
    served
        .first_event(
            &from_the_start(&followed_from(&page).expect("followed")),
            |event| event.contains("entry"),
        )
        .await;

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

#[test_log::test(tokio::test)]
async fn serve_fake_seeds_an_ended_benchmark_run_with_leaks_and_trials_without_a_score_and_a_cancelled_one_with_no_score()
 {
    let served = Served::start().await;

    let page = served.page("/benchmark-runs/1").await;

    let shown = text(&page);
    for said in [
        "8 of 8",
        "1 leak",
        "No score. The run's outcome: failed",
        "No score. The scorer gave up",
        "No score. The build of its environment failed.",
        "gh pr view 4 --comments",
    ] {
        assert!(shown.contains(said), "{said:?} is not in {shown}");
    }
    assert_eq!(
        attributes(&page, r#"nav[aria-label="Breadcrumb"] a"#, "href"),
        ["/benchmarks", "/benchmarks/1"]
    );
    let opened = attributes(&page, "#configurations tr[id$=_details] a", "href");
    assert_eq!(opened.len(), 2, "{opened:?}");
    for transcript in opened {
        let read = text(&served.page(&transcript).await);
        assert!(
            read.contains("The board's columns now carry their names.")
                || read.contains("The trial passes, and it read nothing it should not."),
            "{transcript}: {read}"
        );
    }
    let cancelled = served.page("/benchmark-runs/2").await;
    assert!(
        attributes(&cancelled, "#chart button.point", "id").is_empty()
            && text(&cancelled).contains("No results"),
        "{}",
        cancelled.html()
    );
    let trials: Vec<String> = cancelled
        .select(
            &Selector::parse("#configurations tr[id*=_trial-]:not([id$=_details]) > th + td")
                .unwrap(),
        )
        .map(|cell| cell.text().collect::<String>().trim().to_string())
        .collect();
    assert_eq!(trials, ["cancelled", "cancelled", "cancelled", "cancelled"]);
    let facts = text(&cancelled);
    assert!(
        facts.contains("2 of 4") && facts.contains("$0.43"),
        "the run the cancel stopped has ended and counts in the cost: {facts}"
    );
    let stopped: Vec<String> = attributes(&cancelled, "#configurations tr[id$=_details] a", "href");
    assert_eq!(stopped.len(), 1, "{stopped:?}");
    let read = text(&served.page(&stopped[0]).await);
    assert!(
        read.contains("The board's columns are half named when the run stops."),
        "{}: {read}",
        stopped[0]
    );

    served.stop().await;
}
