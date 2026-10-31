use linewright::host::HostName;

/// `docs/specs/monitoring.md`, Commands: a local `Host` is `localhost` or a
/// loopback address, ignoring case, port and a trailing dot.
#[test]
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

#[test]
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

#[test]
fn an_ipv6_host_without_its_closing_bracket_has_no_name() {
    assert_eq!(HostName::parse("[::1"), None);
}
