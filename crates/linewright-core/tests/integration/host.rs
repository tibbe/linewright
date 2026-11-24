use linewright_core::host::{HostName, PublicUrl};
use rstest::rstest;

/// `docs/specs/operation.md`, Monitoring commands: a local `Host` is
/// `localhost` or a loopback address, ignoring case, port and a trailing dot.
#[test_log::test]
fn a_host_is_local_if_it_names_localhost_or_a_loopback_address() {
    let local = [
        "localhost",
        "LOCALHOST.",
        "localhost.:8080",
        "127.0.0.1:8080",
        "127.0.0.2",
        "[::1]:8080",
        "[::1]",
    ];
    let not_local = [
        "evil.example",
        "evil.example:80",
        "localhost.evil.example",
        "evil-localhost",
        "192.0.2.7",
        "[2001:db8::7]",
        "",
    ];

    for host in local {
        assert!(
            HostName::parse(host).is_some_and(|name| name.is_local()),
            "{host:?}"
        );
    }
    for host in not_local {
        assert!(
            !HostName::parse(host).is_some_and(|name| name.is_local()),
            "{host:?}"
        );
    }
}

#[test_log::test]
fn a_hosts_name_ignores_its_case_port_and_trailing_dot() {
    let same = [
        ("Linewright.Example.COM.", "linewright.example.com"),
        ("linewright.example.com:443", "linewright.example.com"),
        ("[2001:DB8::7]:8081", "[2001:db8::7]"),
    ];

    for (host, name) in same {
        assert_eq!(HostName::parse(host), HostName::parse(name), "{host:?}");
    }
    assert_ne!(
        HostName::parse("www.linewright.example.com"),
        HostName::parse("linewright.example.com")
    );
}

#[test_log::test]
fn an_ipv6_host_without_its_closing_bracket_has_no_name() {
    assert_eq!(HostName::parse("[::1"), None);
}

/// `docs/specs/operation.md`, Serving the UI: the web UI answers only a
/// `Host` that is the public URL's host, compared ignoring case, port and a
/// trailing dot.
#[rstest]
#[case::the_same_name("https://Linewright.Example.com", "linewright.example.com", true)]
#[case::the_name_in_another_case_with_a_trailing_dot(
    "https://Linewright.Example.com",
    "LINEWRIGHT.example.COM.",
    true
)]
#[case::the_name_with_the_default_port(
    "https://Linewright.Example.com",
    "linewright.example.com:443",
    true
)]
#[case::the_name_with_another_port(
    "https://Linewright.Example.com",
    "linewright.example.com:8081",
    true
)]
#[case::an_ipv6_address("https://[::1]:8081", "[::1]", true)]
#[case::an_ipv6_address_with_another_port("https://[::1]:8081", "[::1]:9000", true)]
#[case::an_ipv6_address_written_in_full("https://[::1]:8081", "[0:0:0:0:0:0:0:1]", true)]
#[case::an_ipv4_mapped_ipv6_address_written_in_hex(
    "https://[::ffff:127.0.0.1]",
    "[::ffff:7f00:1]",
    true
)]
#[case::another_name("https://linewright.example.com", "evil.example", false)]
#[case::a_subdomain_of_the_name(
    "https://linewright.example.com",
    "www.linewright.example.com",
    false
)]
#[case::localhost("https://linewright.example.com", "localhost", false)]
#[case::a_loopback_ipv4_address("https://linewright.example.com", "127.0.0.1", false)]
#[case::a_loopback_ipv6_address("https://linewright.example.com", "[::1]:8081", false)]
#[test_log::test]
fn the_web_ui_answers_a_host_when(
    #[case] public_url: &str,
    #[case] host: &str,
    #[case] answered: bool,
) {
    let public_url = PublicUrl::parse(public_url).unwrap();
    let host = HostName::parse(host).unwrap();

    assert_eq!(public_url.answers(&host), answered);
}
