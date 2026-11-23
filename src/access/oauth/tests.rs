use super::RefreshState::{Current, Previous, Retired};
use super::*;

#[track_caller]
fn server() -> Server {
    Server::new(PublicUrl::parse("https://lw.example.com").unwrap())
}

#[track_caller]
fn client() -> Client {
    Client {
        id: ClientId::of("c-1".into()),
        name: Some("Claude Code".into()),
        redirect_uris: vec![
            RedirectUri::parse("http://127.0.0.1:33418/callback").unwrap(),
            RedirectUri::parse("https://agent.example/redirect").unwrap(),
        ],
    }
}

fn request() -> AuthorizeRequest {
    AuthorizeRequest {
        response_type: Some("code".into()),
        client_id: Some("c-1".into()),
        redirect_uri: Some("http://127.0.0.1:33418/callback".into()),
        code_challenge: Some(CHALLENGE_OF_RFC.into()),
        code_challenge_method: Some("S256".into()),
        state: Some("xyz".into()),
        resource: Some("https://lw.example.com/mcp".into()),
        repeated: None,
    }
}

/// RFC 7636's example: its verifier and its challenge.
/// <https://www.rfc-editor.org/rfc/rfc7636#appendix-B>
const VERIFIER_OF_RFC: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE_OF_RFC: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

#[test_log::test]
fn the_metadata_give_the_issuer_and_every_url_under_the_public_url() {
    let server = server();

    assert_eq!(
        server.server_metadata(),
        serde_json::json!({
            "issuer": "https://lw.example.com",
            "authorization_endpoint": "https://lw.example.com/oauth/authorize",
            "token_endpoint": "https://lw.example.com/oauth/token",
            "registration_endpoint": "https://lw.example.com/oauth/register",
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"],
            "client_id_metadata_document_supported": true,
            "authorization_response_iss_parameter_supported": true,
        })
    );
    assert_eq!(
        server.resource_metadata(),
        serde_json::json!({
            "resource": "https://lw.example.com/mcp",
            "authorization_servers": ["https://lw.example.com"],
            "bearer_methods_supported": ["header"],
            "resource_name": "Linewright",
        })
    );
}

#[test_log::test]
fn a_request_with_a_registered_redirect_uri_s256_and_the_mcp_server_is_taken() {
    let checked = check_authorize(&request(), Some(&client()), &server()).unwrap();

    assert_eq!(
        checked,
        Checked {
            client: ClientId::of("c-1".into()),
            redirect_uri: RedirectUri::parse("http://127.0.0.1:33418/callback").unwrap(),
            challenge: Challenge::parse(CHALLENGE_OF_RFC).unwrap(),
            state: Some("xyz".into()),
        }
    );
    let without_resource = AuthorizeRequest {
        resource: None,
        ..request()
    };
    assert!(check_authorize(&without_resource, Some(&client()), &server()).is_ok());
}

/// `docs/specs/access.md`, The authorization server, Redirect URIs: a
/// redirect URI matches as the client registered it, not as a URL means
/// the same.
#[test_log::test]
fn a_redirect_uri_matches_exactly_as_registered() {
    let client = Client {
        redirect_uris: vec![RedirectUri::parse("http://127.0.0.1:5000").unwrap()],
        ..client()
    };
    let request = |redirect_uri: &str| AuthorizeRequest {
        redirect_uri: Some(redirect_uri.into()),
        ..request()
    };

    assert!(check_authorize(&request("http://127.0.0.1:5000"), Some(&client), &server()).is_ok());
    assert_eq!(
        check_authorize(&request("http://127.0.0.1:5000/"), Some(&client), &server()),
        Err(AuthorizeError::CannotSendBack(
            CannotSendBack::NotARedirectUri
        ))
    );
}

/// `docs/specs/access.md`, The authorization server, 2a.
#[test_log::test]
fn an_unknown_client_or_an_unregistered_redirect_uri_is_not_sent_back() {
    let cannot = |why| Err(AuthorizeError::CannotSendBack(why));

    assert_eq!(
        check_authorize(&request(), None, &server()),
        cannot(CannotSendBack::UnknownClient)
    );
    let another_client = AuthorizeRequest {
        client_id: Some("c-2".into()),
        ..request()
    };
    assert_eq!(
        check_authorize(&another_client, Some(&client()), &server()),
        cannot(CannotSendBack::UnknownClient)
    );
    for redirect_uri in [
        None,
        Some("http://127.0.0.1:33419/callback"),
        Some("http://127.0.0.1:33418/callback/"),
        Some("http://127.0.0.1:33418/callback?x=1"),
        Some("https://evil.example/redirect"),
    ] {
        let request = AuthorizeRequest {
            redirect_uri: redirect_uri.map(Into::into),
            ..request()
        };
        assert_eq!(
            check_authorize(&request, Some(&client()), &server()),
            cannot(CannotSendBack::NotARedirectUri),
            "{redirect_uri:?}"
        );
    }
    assert_eq!(
        CannotSendBack::UnknownClient.to_string(),
        "Linewright does not know this client."
    );
    assert_eq!(
        CannotSendBack::NotARedirectUri.to_string(),
        "This is not a redirect URI of this client."
    );
}

/// The error a request is sent back with, as its code.
#[track_caller]
fn sent_back(request: &AuthorizeRequest) -> &'static str {
    match check_authorize(request, Some(&client()), &server()) {
        Err(AuthorizeError::SendBack(sent)) => {
            let SentBack {
                redirect_uri,
                state,
                error,
            } = *sent;
            assert_eq!(redirect_uri.as_str(), "http://127.0.0.1:33418/callback");
            assert_eq!(state, request.state);
            error.error
        }
        other => panic!("sent back, not {other:?}"),
    }
}

/// `docs/specs/access.md`, The authorization server, Grants.
#[test_log::test]
fn a_method_other_than_s256_or_no_pkce_is_refused() {
    let plain = AuthorizeRequest {
        code_challenge_method: Some("plain".into()),
        code_challenge: Some(VERIFIER_OF_RFC.into()),
        ..request()
    };
    let no_method = AuthorizeRequest {
        code_challenge_method: None,
        ..request()
    };
    let no_pkce = AuthorizeRequest {
        code_challenge: None,
        code_challenge_method: None,
        ..request()
    };
    let not_a_hash = AuthorizeRequest {
        code_challenge: Some("short".into()),
        ..request()
    };
    let token = AuthorizeRequest {
        response_type: Some("token".into()),
        ..request()
    };

    assert_eq!(sent_back(&plain), "invalid_request");
    assert_eq!(sent_back(&no_method), "invalid_request");
    assert_eq!(sent_back(&no_pkce), "invalid_request");
    assert_eq!(sent_back(&not_a_hash), "invalid_request");
    assert_eq!(sent_back(&token), "unsupported_response_type");
}

/// A parameter given more than once is refused.
/// <https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-13#section-3.1>
#[test_log::test]
fn a_request_reads_from_its_query_and_a_repeated_parameter_is_refused() {
    let query = "response_type=code&client_id=c-1\
        &redirect_uri=http%3A%2F%2F127.0.0.1%3A33418%2Fcallback\
        &code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM\
        &code_challenge_method=S256&state=xyz\
        &resource=https%3A%2F%2Flw.example.com%2Fmcp&scope=anything";
    assert_eq!(AuthorizeRequest::from_query(query), request());

    let twice = |name: &str| AuthorizeRequest::from_query(&format!("{query}&{name}=again"));
    assert_eq!(sent_back(&twice("state")), "invalid_request");
    assert_eq!(sent_back(&twice("resource")), "invalid_request");
    assert_eq!(
        check_authorize(&twice("client_id"), Some(&client()), &server()),
        Err(AuthorizeError::CannotSendBack(
            CannotSendBack::UnknownClient
        ))
    );
    assert_eq!(
        check_authorize(&twice("redirect_uri"), Some(&client()), &server()),
        Err(AuthorizeError::CannotSendBack(
            CannotSendBack::NotARedirectUri
        ))
    );
}

/// `docs/specs/access.md`, The authorization server, Resource.
#[test_log::test]
fn a_resource_other_than_the_mcp_server_is_refused() {
    for resource in [
        "https://lw.example.com/",
        "https://lw.example.com/mcp/",
        "https://other.example/mcp",
        "http://lw.example.com/mcp",
    ] {
        let request = AuthorizeRequest {
            resource: Some(resource.into()),
            ..request()
        };
        assert_eq!(sent_back(&request), "invalid_target", "{resource}");
    }
}

#[test_log::test]
fn sending_back_adds_the_state_and_the_issuer_to_the_redirect_uri() {
    let redirect_uri = RedirectUri::parse("https://agent.example/cb?app=1").unwrap();

    let url = server().sent_back(&redirect_uri, &[("code", "abc")], Some("s t"));

    assert_eq!(
        url.as_str(),
        "https://agent.example/cb?app=1&code=abc&state=s+t&iss=https%3A%2F%2Flw.example.com"
    );
}

/// `docs/specs/access.md`, The authorization server, Redirect URIs.
#[test_log::test]
fn a_redirect_uri_is_https_or_http_to_a_loopback_address() {
    for good in [
        "https://agent.example/cb",
        "http://127.0.0.1:33418/",
        "http://127.0.0.1/callback",
        "http://[::1]:8080/cb",
        "http://localhost/callback",
    ] {
        assert!(RedirectUri::parse(good).is_ok(), "{good}");
    }
    for bad in [
        "http://agent.example/cb",
        "http://10.0.0.1/cb",
        "vscode://callback",
        "https://agent.example/cb#fragment",
        "not a url",
    ] {
        assert!(RedirectUri::parse(bad).is_err(), "{bad}");
    }
}

/// `docs/specs/access.md`, The authorization server, Clients.
#[test_log::test]
fn a_registration_is_public_with_its_redirect_uris_and_refused_with_any_other() {
    let request = |uris: &[&str]| RegistrationRequest {
        redirect_uris: uris.iter().map(ToString::to_string).collect(),
        client_name: Some("My agent".into()),
    };

    let (name, uris) = registration(&request(&[
        "http://127.0.0.1:5000/cb",
        "https://agent.example/cb",
    ]))
    .unwrap();
    assert_eq!(name.as_deref(), Some("My agent"));
    assert_eq!(
        uris.iter().map(RedirectUri::as_str).collect::<Vec<_>>(),
        ["http://127.0.0.1:5000/cb", "https://agent.example/cb"]
    );

    for refused in [
        &["http://127.0.0.1:5000/cb", "http://agent.example/cb"][..],
        &["myapp://cb"],
        &[],
    ] {
        assert_eq!(
            registration(&request(refused)).unwrap_err().error,
            "invalid_redirect_uri",
            "{refused:?}"
        );
    }
    let unnamed = RegistrationRequest {
        client_name: Some(String::new()),
        ..request(&["https://agent.example/cb"])
    };
    assert_eq!(registration(&unnamed).unwrap().0, None);
}

fn at(seconds: i64) -> Timestamp {
    chrono::DateTime::UNIX_EPOCH + TimeDelta::seconds(seconds)
}

const NOW: i64 = 1_000_000;

fn authorization() -> StoredAuthorization {
    StoredAuthorization {
        id: AuthorizationId(7),
        client: ClientId::of("c-1".into()),
        ends_at: at(NOW) + TimeDelta::days(30),
        ended: false,
    }
}

fn code(used: bool) -> Stored {
    Stored::Code {
        authorization: authorization(),
        challenge: Challenge::parse(CHALLENGE_OF_RFC).unwrap(),
        redirect_uri: RedirectUri::parse("http://127.0.0.1:33418/callback").unwrap(),
        expires_at: at(NOW) + TimeDelta::minutes(10),
        used,
    }
}

fn code_request(verifier: Option<&str>) -> TokenRequest {
    TokenRequest {
        grant: Grant::Code {
            code: "the-code".into(),
            redirect_uri: Some("http://127.0.0.1:33418/callback".into()),
            verifier: verifier.map(Into::into),
        },
        client_id: ClientId::of("c-1".into()),
        resource: Some("https://lw.example.com/mcp".into()),
    }
}

fn refresh_request() -> TokenRequest {
    TokenRequest {
        grant: Grant::Refresh {
            token: "the-token".into(),
        },
        client_id: ClientId::of("c-1".into()),
        resource: None,
    }
}

fn refresh(state: RefreshState) -> Stored {
    Stored::Refresh {
        authorization: authorization(),
        state,
    }
}

/// The error code a decision refuses with.
#[track_caller]
fn refused(decision: TokenDecision) -> &'static str {
    match decision {
        TokenDecision::Refuse(error) => error.error,
        other @ (TokenDecision::Issue { .. } | TokenDecision::End { .. }) => {
            panic!("refused, not {other:?}")
        }
    }
}

fn issued(used: Used) -> TokenDecision {
    TokenDecision::Issue {
        authorization: AuthorizationId(7),
        used,
        access_until: at(NOW) + TimeDelta::hours(1),
    }
}

/// `docs/specs/access.md`, The authorization server, 6.
#[test_log::test]
fn a_code_with_its_verifier_is_exchanged_once() {
    let request = code_request(Some(VERIFIER_OF_RFC));

    assert_eq!(
        exchange(&request, &code(false), at(NOW), &server()),
        issued(Used::Code)
    );
    assert_eq!(
        refused(exchange(&request, &code(true), at(NOW), &server())),
        "invalid_grant"
    );
    assert_eq!(
        refused(exchange(&request, &Stored::Unknown, at(NOW), &server())),
        "invalid_grant"
    );
}

#[test_log::test]
fn a_wrong_or_missing_verifier_is_refused() {
    let wrong = code_request(Some(&"a".repeat(43)));

    assert_eq!(
        refused(exchange(&wrong, &code(false), at(NOW), &server())),
        "invalid_grant"
    );
    assert_eq!(
        refused(exchange(
            &code_request(None),
            &code(false),
            at(NOW),
            &server()
        )),
        "invalid_request"
    );
}

#[test_log::test]
fn a_code_is_refused_to_another_client_after_it_expires_or_to_another_redirect_uri() {
    let request = code_request(Some(VERIFIER_OF_RFC));
    let other_client = TokenRequest {
        client_id: ClientId::of("c-2".into()),
        ..request.clone()
    };
    let other_redirect = TokenRequest {
        grant: Grant::Code {
            code: "the-code".into(),
            redirect_uri: Some("https://agent.example/redirect".into()),
            verifier: Some(VERIFIER_OF_RFC.into()),
        },
        ..request.clone()
    };

    for (refused_request, now) in [
        (&other_client, at(NOW)),
        (&request, at(NOW) + TimeDelta::minutes(10)),
        (&other_redirect, at(NOW)),
    ] {
        assert_eq!(
            refused(exchange(refused_request, &code(false), now, &server())),
            "invalid_grant"
        );
    }
}

/// `docs/specs/access.md`, The authorization server, Resource.
#[test_log::test]
fn a_token_for_another_resource_is_refused() {
    let request = TokenRequest {
        resource: Some("https://other.example/mcp".into()),
        ..code_request(Some(VERIFIER_OF_RFC))
    };

    assert_eq!(
        refused(exchange(&request, &code(false), at(NOW), &server())),
        "invalid_target"
    );
}

/// `docs/specs/access.md`, Tokens, Rotation and A lost response.
#[test_log::test]
fn a_refresh_token_rotates_and_a_lost_response_is_retried_with_the_previous_one() {
    let request = refresh_request();

    assert_eq!(
        exchange(
            &request,
            &refresh(RefreshState::Current),
            at(NOW),
            &server()
        ),
        issued(Used::Current)
    );
    assert_eq!(
        exchange(
            &request,
            &refresh(RefreshState::Previous),
            at(NOW),
            &server()
        ),
        issued(Used::Previous)
    );
}

/// The state each refresh token of an authorization is in after `used`,
/// of those in `states`, with the new current one last.
fn rotated(states: &[RefreshState], used: Used) -> Vec<RefreshState> {
    let mut rotated: Vec<_> = states.iter().map(|state| used.turns(*state)).collect();
    rotated.push(RefreshState::Current);
    rotated
}

/// `docs/specs/access.md`, Tokens: a refresh token works until the client
/// first uses a token that replaced it, and a client that lost its new
/// tokens uses the one before again.
#[test_log::test]
fn the_previous_token_stops_working_once_its_replacement_is_used() {
    let first = rotated(&[], Used::Code);
    assert_eq!(first, [Current], "a code gives the first");

    let refreshed = rotated(&first, Used::Current);
    assert_eq!(
        refreshed,
        [Previous, Current],
        "the first works until its replacement is used"
    );
    let lost = rotated(&refreshed, Used::Previous);
    assert_eq!(
        lost,
        [Previous, Retired, Current],
        "the lost response's token retires"
    );
    let used = rotated(&lost, Used::Current);
    assert_eq!(
        used,
        [Retired, Retired, Previous, Current],
        "the first stops working once its replacement is used"
    );
    assert!(matches!(
        exchange(&refresh_request(), &refresh(used[0]), at(NOW), &server()),
        TokenDecision::End { .. }
    ));
}

/// `docs/specs/access.md`, Tokens: a refresh token stops working once the
/// client first uses the token that replaced it, and using it then ends its
/// authorization.
#[test_log::test]
fn presenting_a_refresh_token_that_stopped_working_ends_its_authorization() {
    let decision = exchange(
        &refresh_request(),
        &refresh(RefreshState::Retired),
        at(NOW),
        &server(),
    );

    match decision {
        TokenDecision::End {
            authorization,
            error,
        } => {
            assert_eq!(authorization, AuthorizationId(7));
            assert_eq!(error.error, "invalid_grant");
        }
        other @ (TokenDecision::Issue { .. } | TokenDecision::Refuse(_)) => {
            panic!("ended, not {other:?}")
        }
    }
    let another_client = TokenRequest {
        client_id: ClientId::of("c-2".into()),
        ..refresh_request()
    };
    assert_eq!(
        refused(exchange(
            &another_client,
            &refresh(RefreshState::Retired),
            at(NOW),
            &server()
        )),
        "invalid_grant",
        "another client's presenting it ends nothing"
    );
    assert_eq!(
        refused(exchange(
            &refresh_request(),
            &Stored::Unknown,
            at(NOW),
            &server()
        )),
        "invalid_grant"
    );
}

/// `docs/specs/access.md`, Tokens, and The authorization server, How long
/// it lasts.
#[test_log::test]
fn an_access_token_lasts_an_hour_and_an_authorization_30_days_from_its_sign_in() {
    let signed_in_at = at(NOW) - TimeDelta::minutes(5);
    assert_eq!(
        authorization_ends(signed_in_at),
        signed_in_at + TimeDelta::days(30)
    );
    let ends_at = at(NOW) + TimeDelta::days(30);
    let request = refresh_request();

    assert_eq!(
        exchange(
            &request,
            &refresh(RefreshState::Current),
            at(NOW),
            &server()
        ),
        issued(Used::Current),
        "an hour from now"
    );
    let near_the_end = ends_at - TimeDelta::minutes(20);
    assert_eq!(
        exchange(
            &request,
            &refresh(RefreshState::Current),
            near_the_end,
            &server()
        ),
        TokenDecision::Issue {
            authorization: AuthorizationId(7),
            used: Used::Current,
            access_until: ends_at,
        },
        "no later than the authorization"
    );
    assert_eq!(
        refused(exchange(
            &request,
            &refresh(RefreshState::Current),
            ends_at,
            &server()
        )),
        "invalid_grant"
    );
    let ended = Stored::Refresh {
        authorization: StoredAuthorization {
            ended: true,
            ..authorization()
        },
        state: RefreshState::Current,
    };
    assert_eq!(
        refused(exchange(&request, &ended, at(NOW), &server())),
        "invalid_grant"
    );
}

#[test_log::test]
fn a_token_request_names_its_client_and_a_grant_linewright_takes() {
    let form = |pairs: &[(&str, &str)]| -> TokenForm {
        serde_urlencoded::from_str(&serde_urlencoded::to_string(pairs).unwrap()).unwrap()
    };

    assert_eq!(
        TokenRequest::parse(form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", "r"),
            ("client_id", "c-1"),
        ])),
        Ok(TokenRequest {
            grant: Grant::Refresh { token: "r".into() },
            client_id: ClientId::of("c-1".into()),
            resource: None,
        })
    );
    let error = |pairs| TokenRequest::parse(form(pairs)).unwrap_err().error;
    assert_eq!(
        error(&[("grant_type", "client_credentials"), ("client_id", "c-1")]),
        "unsupported_grant_type"
    );
    assert_eq!(
        error(&[("grant_type", "password"), ("client_id", "c-1")]),
        "unsupported_grant_type"
    );
    assert_eq!(
        error(&[("grant_type", "refresh_token"), ("refresh_token", "r")]),
        "invalid_request"
    );
    assert_eq!(
        error(&[("grant_type", "authorization_code"), ("client_id", "c-1")]),
        "invalid_request"
    );
}

/// `docs/specs/access.md`, Tokens: an access token stops working one hour
/// after Linewright issues it, or when its authorization ends.
#[test_log::test]
fn an_access_token_works_until_it_expires_or_its_authorization_ends() {
    let expires_at = at(NOW) + TimeDelta::hours(1);
    let ended = StoredAuthorization {
        ended: true,
        ..authorization()
    };
    let over = StoredAuthorization {
        ends_at: at(NOW) + TimeDelta::minutes(30),
        ..authorization()
    };

    assert!(access_works(expires_at, &authorization(), at(NOW)));
    assert!(access_works(
        expires_at,
        &authorization(),
        expires_at - TimeDelta::seconds(1)
    ));
    assert!(!access_works(expires_at, &authorization(), expires_at));
    assert!(!access_works(expires_at, &ended, at(NOW)));
    assert!(!access_works(
        expires_at,
        &over,
        at(NOW) + TimeDelta::minutes(30)
    ));
}
