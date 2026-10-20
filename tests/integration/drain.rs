use crate::common;

use std::time::Duration;

use chrono::TimeDelta;
use common::*;
use linewright::domain::{Login, Number};
use linewright::factory::{Factory, ServeEnd};
use linewright::harness::SessionError;
use linewright::runs::{Ending, Runs, State};
use linewright::webhook::Pending;

struct Served {
    world: World,
    serving: Serving,
}

impl Served {
    /// Serves `world`'s repository with a factory of its own that waits out
    /// the usage limit, as `serve` does, and waits for its first poll.
    async fn start(world: World) -> Served {
        let factory = Factory::start(
            world.store.clone(),
            world.harness.clone(),
            world.clock.clone(),
            linewright::factory::FactoryConfig {
                waits_out_usage_limit: true,
                ..config(&world.dir.path().join("work"))
            },
        )
        .await
        .unwrap();
        let serving = Serving::start(factory, Duration::from_millis(10), Pending::default());
        let mut served = serving.served.clone();
        tokio::time::timeout(Duration::from_secs(10), served.wait_for(Option::is_some))
            .await
            .expect("the first poll")
            .unwrap();
        Served { world, serving }
    }

    fn runs_in_flight(&self) -> usize {
        self.serving
            .served
            .borrow()
            .as_ref()
            .map_or(0, |served| served.view_now().in_flight.len())
    }

    fn draining(&self) -> bool {
        self.serving
            .served
            .borrow()
            .as_ref()
            .is_some_and(|served| served.view_now().memory.drain.is_some())
    }

    /// Lets serving poll a few times more.
    async fn polls_go_on(&self) {
        let mut served = self.serving.served.clone();
        for _ in 0..5 {
            tokio::time::timeout(Duration::from_secs(10), served.changed())
                .await
                .expect("a poll")
                .unwrap();
        }
    }
}

fn ended_runs(world: &World) -> Vec<Ending> {
    Runs::new(&world.dir.path().join("work"))
        .read()
        .unwrap()
        .into_iter()
        .map(|recorded| match recorded.state {
            State::Ended { ending, .. } => ending,
            state @ (State::InFlight | State::Interrupted) => {
                panic!("a run is not over: {state:?}")
            }
        })
        .collect()
}

fn open_ticket(world: &World, title: &str) -> Number {
    world.store.open_issue(
        "maintainer",
        title,
        "Please.",
        &["ready-for-agent", "linewright:designed"],
    )
}

fn info_with(log: &Log, phrase: &str) -> Vec<String> {
    log.lines("INFO")
        .into_iter()
        .filter(|line| line.contains(phrase))
        .collect()
}

#[tokio::test]
async fn a_drain_with_no_run_in_flight_ends_serving_at_once() {
    let (log, _guard) = Log::capture();
    let served = Served::start(World::new().await).await;

    served.serving.drain();

    assert_eq!(served.serving.ended().await, ServeEnd::Drained);
    assert_eq!(info_with(&log, "drain begins").len(), 1, "{}", log.text());
    assert_eq!(info_with(&log, "drained").len(), 1, "{}", log.text());
}

#[tokio::test]
async fn a_drain_waits_for_the_run_in_flight_which_ends_as_it_would_have() {
    let (log, _guard) = Log::capture();
    let world = World::new().await;
    implement(&world);
    let agent = world.harness.hold();
    let ticket = open_ticket(&world, "Do the thing");
    let served = Served::start(world).await;
    eventually(|| served.runs_in_flight() == 1).await;

    served.serving.drain();
    eventually(|| served.draining()).await;
    let held_back = open_ticket(&served.world, "Do another thing");
    let mislabeled = served.world.store.open_issue(
        "maintainer",
        "Two labels",
        "Which?",
        &["needs-info", "ready-for-human"],
    );
    served.polls_go_on().await;

    assert!(!served.serving.has_ended());
    assert_eq!(served.world.issue(held_back).assignees, []);
    assert_eq!(served.world.issue(mislabeled).labels.len(), 1);
    let begins = info_with(&log, "drain begins");
    assert!(
        begins.len() == 1 && begins[0].contains("1/implement-1"),
        "{begins:?}"
    );

    drop(agent);

    let Served {
        world: ended,
        serving,
    } = served;
    assert_eq!(serving.ended().await, ServeEnd::Drained);
    assert_eq!(ended_runs(&ended), [Ending::Done]);
    assert_eq!(ended.store.pulls().len(), 1);
    assert!(ended.issue(ticket).is_assigned(&Login::new(LINEWRIGHT)));
    assert_eq!(ended.issue(held_back).assignees, []);
    assert_eq!(info_with(&log, "drained").len(), 1, "{}", log.text());
}

#[tokio::test]
async fn a_drain_waits_for_a_run_waiting_out_the_usage_limit() {
    let world = World::new().await;
    let refused = std::sync::atomic::AtomicBool::new(false);
    world
        .harness
        .on(linewright::reconcile::WorkKind::Implement, move |call| {
            if !refused.swap(true, std::sync::atomic::Ordering::Relaxed) {
                return refused_part_way(call);
            }
            commit_in(&call.spec.dir, &[("src/thing.txt", "done\n")], "Do it");
            Ok::<_, SessionError>(
                serde_json::json!({"outcome": "done", "pr_title": "Do it", "pr_body": "Does it."})
                    .to_string(),
            )
        });
    open_ticket(&world, "Do the thing");
    let served = Served::start(world).await;
    assert_eq!(
        served.world.clock.next_sleep().await,
        usage_limit_resets_at()
    );

    served.serving.drain();
    eventually(|| served.draining()).await;
    served.polls_go_on().await;

    assert!(!served.serving.has_ended());

    let now = linewright::clock::Clock::now(served.world.clock.as_ref());
    served
        .world
        .clock
        .advance(usage_limit_resets_at() - now + TimeDelta::minutes(1));

    let Served {
        world: ended,
        serving,
    } = served;
    assert_eq!(serving.ended().await, ServeEnd::Drained);
    assert_eq!(ended_runs(&ended), [Ending::Done]);
}

#[tokio::test]
async fn a_stop_while_draining_cancels_the_runs_in_flight() {
    let world = World::new().await;
    implement(&world);
    let agent = world.harness.hold();
    open_ticket(&world, "Do the thing");
    let served = Served::start(world).await;
    eventually(|| served.runs_in_flight() == 1).await;
    served.serving.drain();
    eventually(|| served.draining()).await;

    let stop = tokio::spawn(served.serving.stop());

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), stop)
            .await
            .expect("serving stops")
            .unwrap(),
        ServeEnd::Stop
    );
    drop(agent);
    let runs = Runs::new(&served.world.dir.path().join("work"));
    let ended: Vec<State> = runs.read().unwrap().into_iter().map(|r| r.state).collect();
    assert!(
        matches!(
            ended.as_slice(),
            [State::Ended {
                ending: Ending::Cancelled,
                ..
            }]
        ),
        "{ended:?}"
    );
}
