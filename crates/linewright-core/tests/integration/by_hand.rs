use linewright_core::automation::{self, Automation, AutomationFiles, AutomationListing, Unparsed};
use linewright_core::by_hand::{HandRefusal, take};
use linewright_core::repo_files::FileError;

fn weekly() -> Automation {
    automation::parse(
        &"weekly.md".parse().unwrap(),
        "---\non:\n  schedule: \"0 6 * * 1\"\n---\nLook.\n",
    )
    .unwrap()
}

/// The automation files `weekly.md`, which parses, and `broken.md`, which
/// does not.
fn listed() -> AutomationListing {
    AutomationListing::Listed(AutomationFiles {
        parsed: vec![weekly()],
        unparsed: vec![Unparsed {
            path: ".linewright/automations/broken.md".into(),
            reason: "it has no front matter".into(),
        }],
    })
}

#[test_log::test]
fn a_request_for_an_automation_that_parsed_is_taken_for_it() {
    let listing = listed();

    assert_eq!(
        take(&"weekly.md".parse().unwrap(), false, &listing),
        Ok(&weekly())
    );
}

#[test_log::test]
fn a_request_before_the_first_poll_has_listed_the_automations_is_refused() {
    let refused = take(
        &"weekly.md".parse().unwrap(),
        false,
        &AutomationListing::NotYet,
    )
    .unwrap_err();

    assert_eq!(refused, HandRefusal::BeforeFirstPoll);
    assert_eq!(
        refused.to_string(),
        "`serve` can't run an automation before its first poll has listed the automations"
    );
}

#[test_log::test]
fn a_request_while_the_latest_poll_could_not_list_the_automations_is_refused_saying_why() {
    let unlisted = AutomationListing::Unlisted {
        why: "GitHub answered 502".into(),
        last: None,
    };

    let refused = take(&"weekly.md".parse().unwrap(), false, &unlisted).unwrap_err();

    assert_eq!(
        refused.to_string(),
        "`serve`'s latest poll could not list the automations: GitHub answered 502"
    );
}

#[test_log::test]
fn a_request_during_a_drain_is_refused_before_the_automations_are_listed() {
    let refused = take(
        &"weekly.md".parse().unwrap(),
        true,
        &AutomationListing::NotYet,
    )
    .unwrap_err();

    assert_eq!(refused, HandRefusal::Draining);
}

#[test_log::test]
fn a_request_during_a_drain_is_refused() {
    let listing = listed();

    let refused = take(&"weekly.md".parse().unwrap(), true, &listing).unwrap_err();

    assert_eq!(refused, HandRefusal::Draining);
    assert_eq!(
        refused.to_string(),
        "`serve` is draining: it takes no request to run an automation"
    );
}

#[test_log::test]
fn a_request_for_no_automation_is_refused_naming_its_file() {
    let listing = listed();

    let refused = take(&"daily.md".parse().unwrap(), false, &listing).unwrap_err();

    assert_eq!(
        refused.to_string(),
        "there is no automation `daily.md`: the default branch has no \
         .linewright/automations/daily.md"
    );
}

#[test_log::test]
fn a_request_for_a_file_that_does_not_parse_is_refused_with_its_error() {
    let listing = listed();

    let refused = take(&"broken.md".parse().unwrap(), false, &listing).unwrap_err();

    assert_eq!(
        refused,
        HandRefusal::Unparsed(FileError {
            path: ".linewright/automations/broken.md".into(),
            error: "it has no front matter".into(),
        })
    );
    assert_eq!(
        refused.to_string(),
        ".linewright/automations/broken.md does not parse: it has no front matter"
    );
}
