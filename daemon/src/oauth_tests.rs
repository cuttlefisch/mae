//! Tests for [`super`] — the OAuth 2.1 resource server (ADR-052).
//!
//! Extracted under CLAUDE.md's file-ceiling remedy: `oauth.rs` was already 8.6%
//! over its recorded baseline before #456's quota wiring, so trimming the new
//! lines was a treadmill rather than a fix. Same `watch.rs` / `watch_tests.rs`
//! precedent; `#[path]` adds a module level, so the inner `mod tests` uses
//! `super::super::*`.

#[cfg(test)]
mod tests {
    use super::super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::traits::PublicKeyParts;
    use rsa::RsaPrivateKey;

    /// Audit #588.3 — no response on this listener carried `Cache-Control`.
    /// It serves bearer-authenticated KB content, so an absent `no-store` lets
    /// a browser persist it to its on-disk cache and lets an intermediary
    /// retain it. RFC 6749 §5.1 (carried
    /// forward by OAuth 2.1) requires it.
    ///
    /// Covers every status class this module emits — success, the PRM
    /// document, 401, 403 — because a helper that only hardened the happy
    /// path would leave the error bodies (which echo request details back)
    /// cacheable.
    #[test]
    fn every_response_is_marked_no_store() {
        let config = base_config();

        let cases: Vec<(&str, Response<Full<Bytes>>)> = vec![
            (
                "200 json",
                json_response(StatusCode::OK, serde_json::json!({"ok": true})),
            ),
            (
                "403 json",
                json_response(
                    StatusCode::FORBIDDEN,
                    serde_json::json!({"error": "access_denied"}),
                ),
            ),
            ("401 unauthorized", unauthorized(&config, "no token")),
        ];

        for (label, resp) in cases {
            let headers = resp.headers();
            assert_eq!(
                headers
                    .get(hyper::header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok()),
                Some("no-store"),
                "{label}: missing Cache-Control: no-store"
            );
            assert_eq!(
                headers
                    .get(hyper::header::PRAGMA)
                    .and_then(|v| v.to_str().ok()),
                Some("no-cache"),
                "{label}: missing Pragma: no-cache"
            );
        }

        // The 401 must keep its WWW-Authenticate challenge — the header helper
        // must not have clobbered the header it is layered on top of.
        let resp = unauthorized(&config, "no token");
        assert!(
            resp.headers().contains_key(hyper::header::WWW_AUTHENTICATE),
            "the challenge header must survive the no-store wrapper"
        );
    }

    const TEST_KID: &str = "test-key-1";
    const TEST_RESOURCE: &str = "https://mae.example.com/mcp";

    /// Generates a fresh RSA keypair and returns (PEM-encoded private key
    /// for signing test tokens, the JWK this module's validator consumes).
    /// Fresh per test (never a hardcoded/shared key -- CLAUDE.md principle
    /// #14's "real inputs, not unicorn values") so no test accidentally
    /// depends on key material another test also uses.
    fn generate_test_key() -> (String, JwkOwned) {
        // Use the RNG from `rsa`'s OWN `rand_core`, not the workspace `rand`.
        // The daemon is on rand 0.10 (rand_core 0.10) while `rsa` still wants
        // rand_core 0.6's traits, so a `rand::rng()` handle does not satisfy
        // `CryptoRngCore` -- two rand_core versions in one graph. `OsRng` is also
        // the right choice on merit for key generation.
        let mut rng = rsa::rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("RSA keygen");
        let pem = private_key
            .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
            .expect("PEM encode")
            .to_string();
        let public_key = private_key.to_public_key();
        let n = base64_url(&public_key.n().to_bytes_be());
        let e = base64_url(&public_key.e().to_bytes_be());
        (
            pem,
            JwkOwned {
                kid: TEST_KID.to_string(),
                n,
                e,
            },
        )
    }

    fn base64_url(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    }

    fn sign_token(private_key_pem: &str, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(TEST_KID.to_string());
        let encoding_key =
            EncodingKey::from_rsa_pem(private_key_pem.as_bytes()).expect("valid PEM");
        encode(&header, claims, &encoding_key).expect("sign")
    }

    fn base_config() -> ResourceServerConfig {
        ResourceServerConfig {
            canonical_resource_uri: TEST_RESOURCE.to_string(),
            principal_claim: "sub".to_string(),
            groups_claim: "groups".to_string(),
            allowed_groups: Vec::new(),
            jwks_url: "https://unused-in-these-tests.example.com/jwks".to_string(),
            jwks_ca_file: None,
            issuer: Some("https://idp.example.com".to_string()),
            kb_query_enabled: false,
            site_enabled: false,
            site_locale: "en-US".to_string(),
            site_hidden_namespaces: Vec::new(),
            site_home: std::collections::BTreeMap::new(),
            site_title: std::collections::BTreeMap::new(),
            max_request_body_bytes: 1_048_576,
            kb_query_max_body_bytes: 65_536,
            kb_query_max_scan_nodes: 500,
            kb_query_max_search_results: 20,
            second_factor: Default::default(),
        }
    }

    fn valid_claims(now: u64) -> serde_json::Value {
        serde_json::json!({
            "sub": "alice@example.com",
            "aud": TEST_RESOURCE,
            "iss": "https://idp.example.com",
            "iat": now,
            "exp": now + 3600,
        })
    }

    fn now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn valid_token_is_accepted_and_principal_mapped() {
        let (pem, jwk) = generate_test_key();
        let token = sign_token(&pem, &valid_claims(now_unix()));
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        let principal = result.expect("valid token must be accepted");
        // ADR-098 Amendment 1, C1: the claim is the subject, never the
        // principal; until a binding resolves it, it is an unbound subject.
        assert_eq!(principal.subject, "alice@example.com");
        assert_eq!(
            principal.principal,
            mae_daemon::oauth_self_issue::unbound_principal(
                "https://idp.example.com",
                "alice@example.com"
            )
        );
        assert_eq!(
            principal.credential,
            mae_daemon::oauth_self_issue::Credential::IdpUnbound
        );
        assert_eq!(principal.audience, vec![TEST_RESOURCE.to_string()]);
    }

    /// Adversarial (required by ADR-052): wrong-audience token rejected --
    /// including the confused-deputy case of a validly-signed token minted
    /// for a genuinely DIFFERENT resource server.
    #[test]
    fn token_for_a_different_resource_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let mut claims = valid_claims(now_unix());
        claims["aud"] =
            serde_json::json!("https://a-completely-different-mcp-server.example.com/mcp");
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::WrongAudience));
    }

    /// Adversarial (required): expired token rejected.
    #[test]
    fn expired_token_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let now = now_unix();
        let mut claims = valid_claims(now);
        claims["exp"] = serde_json::json!(now.saturating_sub(3600));
        claims["iat"] = serde_json::json!(now.saturating_sub(7200));
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::Expired));
    }

    /// Adversarial (required): a tampered/forged signature is rejected --
    /// the token is signed by a DIFFERENT key than the one in the server's
    /// JWKS (simulating either a forgery attempt or a rotated-out key,
    /// this module's stateless equivalent of "revoked").
    #[test]
    fn token_signed_by_an_unknown_key_is_rejected() {
        let (attacker_pem, _attacker_jwk) = generate_test_key();
        let (_server_pem, server_jwk) = generate_test_key();
        // Attacker signs with their own key but claims the SERVER's kid,
        // attempting to pass off a forged token as legitimately signed.
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(server_jwk.kid.clone());
        let encoding_key = EncodingKey::from_rsa_pem(attacker_pem.as_bytes()).expect("valid PEM");
        let token =
            jsonwebtoken::encode(&header, &valid_claims(now_unix()), &encoding_key).expect("sign");

        let result = validate_bearer_token(&token, &[server_jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::InvalidSignature));
    }

    #[test]
    fn token_with_kid_absent_from_jwks_is_rejected() {
        let (pem, mut jwk) = generate_test_key();
        let token = sign_token(&pem, &valid_claims(now_unix()));
        jwk.kid = "a-different-kid-than-the-token-used".to_string();
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::UnknownKey));
    }

    #[test]
    fn wrong_issuer_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let mut claims = valid_claims(now_unix());
        claims["iss"] = serde_json::json!("https://a-different-idp.example.com");
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::WrongIssuer));
    }

    #[test]
    fn malformed_token_is_rejected_not_panicking() {
        let (_, jwk) = generate_test_key();
        let result = validate_bearer_token("not.a.jwt", &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::Malformed));
    }

    /// Adversarial (jsonwebtoken 12, "fix remaining type confusion with sub,
    /// iss, aud"): a correctly signed token whose `aud` is not a string or a
    /// string array is refused. Through jsonwebtoken 11 such a claim parsed
    /// as "malformed", fell through every arm of the audience match, and the
    /// token was ACCEPTED for whatever resource it was presented to -- the
    /// mixed array even reached the principal carrying this resource's URI.
    /// The same key and claims with a string `aud` are accepted first, so a
    /// refusal can only come from the claim's type. Since `aud` is a required
    /// claim, jsonwebtoken may report an unparseable one as missing, which the
    /// validator maps to `WrongAudience`; either refusal closes the hole, so the
    /// oracle is "refused as malformed or as not this resource", never accepted.
    #[test]
    fn a_non_string_audience_is_refused() {
        let (pem, jwk) = generate_test_key();
        let config = base_config();
        let control = sign_token(&pem, &valid_claims(now_unix()));
        assert!(validate_bearer_token(&control, std::slice::from_ref(&jwk), &config).is_ok());
        for aud in [
            serde_json::json!(1234),
            serde_json::json!({ "uri": TEST_RESOURCE }),
            serde_json::json!([TEST_RESOURCE, 1]),
            serde_json::json!(true),
        ] {
            let mut claims = valid_claims(now_unix());
            claims["aud"] = aud.clone();
            let token = sign_token(&pem, &claims);
            let got = validate_bearer_token(&token, std::slice::from_ref(&jwk), &config);
            assert!(
                matches!(
                    got,
                    Err(TokenValidationError::Malformed | TokenValidationError::WrongAudience)
                ),
                "aud = {aud}: {got:?}"
            );
        }
    }

    /// Adversarial (jsonwebtoken 12, "reject unknown critical extensions"):
    /// RFC 7515 §4.1.11 says a recipient that does not understand a `crit`
    /// header parameter MUST reject the token. Through jsonwebtoken 11 a
    /// correctly signed token naming an extension nobody here implements was
    /// accepted. The control is the same key and claims without `crit`.
    #[test]
    fn an_unknown_critical_header_is_refused() {
        let (pem, jwk) = generate_test_key();
        let config = base_config();
        let key = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("valid PEM");
        let sign = |crit: Option<Vec<String>>| {
            let mut header = Header::new(Algorithm::RS256);
            header.kid = Some(TEST_KID.to_string());
            header.crit = crit;
            encode(&header, &valid_claims(now_unix()), &key).expect("sign")
        };
        assert!(validate_bearer_token(&sign(None), std::slice::from_ref(&jwk), &config).is_ok());
        for crit in [vec!["https://example.com/must-understand"], vec![]] {
            let token = sign(Some(crit.iter().map(|s| s.to_string()).collect()));
            assert_eq!(
                validate_bearer_token(&token, std::slice::from_ref(&jwk), &config),
                Err(TokenValidationError::Malformed),
                "crit = {crit:?}"
            );
        }
    }

    /// Adversarial (ADR-052, RFC 8707 confused deputy): a correctly signed token
    /// with NO `aud` claim is refused as the wrong audience. jsonwebtoken checks
    /// the audience only when the claim is present, so before `aud` was required a
    /// token the IdP minted for another client was accepted here. Controls: the
    /// same claims with this resource as `aud` (string, and an array that also
    /// names a client id, as real IdPs issue) are accepted.
    #[test]
    fn a_token_without_an_audience_is_refused() {
        let (pem, jwk) = generate_test_key();
        let config = base_config();
        let keys = std::slice::from_ref(&jwk);
        let with = |aud: Option<serde_json::Value>| {
            let mut claims = valid_claims(now_unix());
            match aud {
                Some(a) => claims["aud"] = a,
                None => {
                    claims.as_object_mut().unwrap().remove("aud");
                }
            }
            validate_bearer_token(&sign_token(&pem, &claims), keys, &config)
        };
        assert!(with(Some(serde_json::json!(TEST_RESOURCE))).is_ok());
        assert!(with(Some(serde_json::json!(["some-client-id", TEST_RESOURCE]))).is_ok());
        assert_eq!(with(None), Err(TokenValidationError::WrongAudience));
        // The empty forms an IdP or an attacker might send are no audience either.
        for empty in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!(""),
        ] {
            assert_eq!(
                with(Some(empty.clone())),
                Err(TokenValidationError::WrongAudience),
                "aud = {empty}"
            );
        }
    }

    #[test]
    fn missing_principal_claim_is_rejected() {
        let (pem, jwk) = generate_test_key();
        let mut claims = valid_claims(now_unix());
        claims.as_object_mut().unwrap().remove("sub");
        let token = sign_token(&pem, &claims);
        let result = validate_bearer_token(&token, &[jwk], &base_config());
        assert_eq!(result, Err(TokenValidationError::MissingPrincipalClaim));
    }

    #[test]
    fn protected_resource_metadata_names_the_configured_authorization_server() {
        let metadata = protected_resource_metadata(&base_config());
        assert_eq!(metadata["resource"], TEST_RESOURCE);
        assert_eq!(
            metadata["authorization_servers"][0],
            "https://idp.example.com"
        );
    }

    // --- Request body size limiting (QA-pass finding, principle #15) ---
    //
    // These exercise the exact same `http_body_util::Limited` +
    // `BodyExt::collect` + `LengthLimitError` downcast triple `handle_request`
    // uses, against a concrete `Full` body -- `Incoming` (the real hyper
    // connection body type) can't be constructed outside a live connection,
    // so this is the faithful unit-level proof; a real over-the-wire 413
    // round trip is covered separately by the OAuth/kb-query e2e suite.

    #[tokio::test]
    async fn a_request_body_over_the_configured_limit_is_rejected_before_full_buffering() {
        use http_body_util::{BodyExt, Full, Limited};
        let oversized = Full::new(Bytes::from(vec![b'x'; 200]));
        let limited = Limited::new(oversized, 100);

        let result = BodyExt::collect(limited).await;

        let err = result.expect_err("a body exceeding the limit must error, never fully buffer");
        assert!(
            err.downcast_ref::<http_body_util::LengthLimitError>()
                .is_some(),
            "expected a LengthLimitError specifically, got: {err}"
        );
    }

    /// The 403 for a token without a required group is a contract: the
    /// browser client's backend recognises it by status 403 and
    /// `kind == "group_required"` (with `error` the RFC 6750 code
    /// `insufficient_scope`) and answers its own not-found. A change to any of
    /// the three must be deliberate and announced, so it is pinned here.
    #[tokio::test]
    async fn the_group_refusal_body_is_a_pinned_contract() {
        use http_body_util::BodyExt;
        let r = group_refusal();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let bytes = r.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "insufficient_scope");
        assert_eq!(body["kind"], "group_required");
        assert!(body["error_description"].is_string());
    }

    #[tokio::test]
    async fn a_request_body_within_the_configured_limit_is_accepted_unchanged() {
        use http_body_util::{BodyExt, Full, Limited};
        let payload = vec![b'x'; 50];
        let body = Full::new(Bytes::from(payload.clone()));
        let limited = Limited::new(body, 100);

        let result = BodyExt::collect(limited).await;

        let collected = result.expect("a body within the limit must be read successfully");
        assert_eq!(collected.to_bytes().as_ref(), payload.as_slice());
    }

    #[test]
    fn www_authenticate_header_points_at_the_prm_document() {
        let header = www_authenticate_header(&base_config());
        assert!(header.contains("Bearer"));
        assert!(header.contains(&format!(
            "{}/.well-known/oauth-protected-resource",
            TEST_RESOURCE
        )));
    }

    /// ADR-098 for ADR-113 B7/C4: how a person signed in, read from real RS256
    /// tokens through the listener's own validator. The fixtures are the shapes
    /// an identity provider was measured to emit: a second factor (`pwd` and
    /// `mfa`), a password alone, and a session started from a recovery link,
    /// which carries no `amr`. Every one carries the same constant `acr`, which
    /// is why `acr` alone cannot tell them apart.
    mod sign_in_claims {
        use super::*;
        use mae_daemon::sign_in::{SecondFactor, SecondFactorRule, SignIn, ACR, AMR};

        const CONSTANT_ACR: &str = "constant-acr";

        fn claims_with(now: u64, extra: serde_json::Value) -> serde_json::Value {
            let mut c = valid_claims(now);
            for (k, v) in extra.as_object().unwrap() {
                c[k] = v.clone();
            }
            c
        }

        fn second_factor_login(now: u64) -> serde_json::Value {
            claims_with(
                now,
                serde_json::json!({"amr": ["pwd", "mfa"], "acr": CONSTANT_ACR,
                                   "auth_time": now - 30, "groups": ["readers"]}),
            )
        }

        fn password_login(now: u64) -> serde_json::Value {
            claims_with(
                now,
                serde_json::json!({"amr": ["pwd"], "acr": CONSTANT_ACR, "auth_time": now - 30}),
            )
        }

        fn recovery_link_login(now: u64) -> serde_json::Value {
            claims_with(
                now,
                serde_json::json!({"acr": CONSTANT_ACR, "auth_time": now - 30}),
            )
        }

        fn sign_in_of(claims: &serde_json::Value, config: &ResourceServerConfig) -> SignIn {
            let (pem, jwk) = generate_test_key();
            let token = sign_token(&pem, claims);
            validate_bearer_token(&token, &[jwk], config)
                .expect("a valid token")
                .sign_in
                .expect("an external token always carries a SignIn")
        }

        fn rule(amr: &[&str], acr: &[&str]) -> SecondFactorRule {
            SecondFactorRule {
                amr: amr.iter().map(|s| s.to_string()).collect(),
                acr: acr.iter().map(|s| s.to_string()).collect(),
            }
        }

        /// No built-in second-factor values: the values differ by identity
        /// provider. Configured, the two keys reach the rule unchanged.
        #[test]
        fn the_rule_has_no_default_and_reads_both_keys() {
            let default = crate::config::OAuthConfig::default();
            assert_eq!(default.second_factor_rule(), SecondFactorRule::default());
            let set: crate::config::OAuthConfig =
                toml::from_str("second_factor_amr = [\"mfa\"]\nsecond_factor_acr = [\"strong\"]")
                    .unwrap();
            assert_eq!(set.second_factor_rule(), rule(&["mfa"], &["strong"]));
        }

        #[test]
        fn the_measured_sign_ins_are_read_as_emitted() {
            let now = now_unix();
            let c = base_config();
            let s = sign_in_of(&second_factor_login(now), &c);
            assert_eq!(s.amr, vec!["pwd", "mfa"]);
            assert_eq!(s.acr.as_deref(), Some(CONSTANT_ACR));
            assert_eq!(s.auth_time, Some(now - 30));
            assert_eq!(s.groups, vec!["readers"]);
            let s = sign_in_of(&password_login(now), &c);
            assert_eq!(s.amr, vec!["pwd"]);
            assert_eq!(s.auth_time, Some(now - 30));
            let s = sign_in_of(&recovery_link_login(now), &c);
            assert!(s.amr.is_empty(), "a recovery-link session has no amr");
            assert_eq!(s.acr.as_deref(), Some(CONSTANT_ACR));
        }

        #[test]
        fn the_configured_groups_claim_is_the_one_read() {
            let now = now_unix();
            let mut c = base_config();
            c.groups_claim = "roles".into();
            let claims = claims_with(
                now,
                serde_json::json!({"roles": "editors", "groups": ["x"]}),
            );
            assert_eq!(sign_in_of(&claims, &c).groups, vec!["editors"]);
        }

        #[test]
        fn malformed_claim_types_are_absent_and_never_pass() {
            let now = now_unix();
            let c = base_config();
            let lenient = rule(&["mfa", "pwd"], &["2", CONSTANT_ACR]);
            for extra in [
                serde_json::json!({"amr": "mfa", "acr": 2, "auth_time": "now"}),
                serde_json::json!({"amr": ["mfa", 7], "acr": [CONSTANT_ACR], "auth_time": -1}),
                serde_json::json!({"amr": {"mfa": true}, "acr": {}, "auth_time": 1.5}),
            ] {
                let s = sign_in_of(&claims_with(now, extra.clone()), &c);
                assert!(s.amr.is_empty(), "{extra}");
                assert_eq!(s.acr, None, "{extra}");
                assert_eq!(s.auth_time, None, "{extra}");
                assert_eq!(s.second_factor(&lenient), None, "{extra}");
                assert!(!s.fresh_within(now, u64::MAX), "{extra}");
            }
        }

        #[test]
        fn with_nothing_configured_no_sign_in_has_a_second_factor() {
            let now = now_unix();
            let c = base_config();
            for claims in [
                second_factor_login(now),
                password_login(now),
                recovery_link_login(now),
            ] {
                let s = sign_in_of(&claims, &c);
                assert_eq!(s.second_factor(&c.second_factor), None, "{claims}");
            }
        }

        #[test]
        fn an_amr_rule_passes_the_second_factor_and_nothing_else() {
            let now = now_unix();
            let c = base_config();
            let r = rule(&["mfa", "hwk", "sc"], &[]);
            assert_eq!(
                sign_in_of(&second_factor_login(now), &c).second_factor(&r),
                Some(SecondFactor {
                    claim: AMR,
                    value: "mfa".into()
                })
            );
            assert_eq!(sign_in_of(&password_login(now), &c).second_factor(&r), None);
            assert_eq!(
                sign_in_of(&recovery_link_login(now), &c).second_factor(&r),
                None
            );
        }

        #[test]
        fn an_acr_rule_matches_the_exact_acr_only() {
            let now = now_unix();
            let c = base_config();
            let strong = claims_with(now, serde_json::json!({"acr": "strong", "amr": ["pwd"]}));
            let r = rule(&[], &["strong"]);
            assert_eq!(
                sign_in_of(&strong, &c).second_factor(&r),
                Some(SecondFactor {
                    claim: ACR,
                    value: "strong".into()
                })
            );
            assert_eq!(sign_in_of(&password_login(now), &c).second_factor(&r), None);
            // The hazard the docs name: a constant `acr`, configured, passes a
            // password-only sign-in. Configure `amr` for such a provider.
            let r = rule(&[], &[CONSTANT_ACR]);
            assert!(sign_in_of(&password_login(now), &c)
                .second_factor(&r)
                .is_some());
        }

        #[test]
        fn freshness_reads_auth_time_from_the_token() {
            let now = now_unix();
            let c = base_config();
            let s = sign_in_of(&second_factor_login(now), &c);
            assert!(s.fresh_within(now, 30));
            assert!(!s.fresh_within(now, 29));
            let leeway = mae_daemon::sign_in::JWT_CLOCK_LEEWAY_SECS;
            let ahead = |by: u64| {
                sign_in_of(
                    &claims_with(now, serde_json::json!({"auth_time": now + by})),
                    &c,
                )
            };
            assert!(ahead(leeway).fresh_within(now, 300));
            assert!(!ahead(leeway + 1).fresh_within(now, 300));
        }

        /// The leeway freshness allows a future `auth_time` is the one the
        /// validator allows `exp`: a token just inside it validates, one just
        /// outside is expired.
        #[test]
        fn the_validator_grants_exp_the_same_leeway() {
            let leeway = mae_daemon::sign_in::JWT_CLOCK_LEEWAY_SECS;
            // Why 60: it is the library's own default, which this validator
            // used before stating it, so stating it changed no token's fate.
            assert_eq!(leeway, Validation::new(Algorithm::RS256).leeway);
            let (pem, jwk) = generate_test_key();
            let now = now_unix();
            let inside = claims_with(now, serde_json::json!({"exp": now + 10 - leeway}));
            let outside = claims_with(now, serde_json::json!({"exp": now - 10 - leeway}));
            let config = base_config();
            let validate = |c: &serde_json::Value| {
                validate_bearer_token(&sign_token(&pem, c), std::slice::from_ref(&jwk), &config)
            };
            assert!(validate(&inside).is_ok());
            assert_eq!(validate(&outside), Err(TokenValidationError::Expired));
        }

        #[tokio::test]
        async fn the_bare_probe_reports_the_sign_in_and_the_rule_it_meets() {
            let now = now_unix();
            let mut c = base_config();
            c.second_factor = rule(&["mfa"], &[]);
            let (pem, jwk) = generate_test_key();
            let token = sign_token(&pem, &second_factor_login(now));
            let vp = validate_bearer_token(&token, &[jwk], &c).unwrap();
            let q = mae_daemon::quota::NoQuota;
            let body = route_authenticated_request(None, &c, None, &vp, &q).await;
            let s = &body["sign_in"];
            assert_eq!(s["amr"], serde_json::json!(["pwd", "mfa"]));
            assert_eq!(s["auth_time"], serde_json::json!(now - 30));
            assert_eq!(
                s["second_factor"],
                serde_json::json!({"claim": "amr", "value": "mfa"})
            );
            c.second_factor = SecondFactorRule::default();
            let body = route_authenticated_request(None, &c, None, &vp, &q).await;
            assert!(body["sign_in"]["second_factor"].is_null());
            let mut self_issued = vp;
            self_issued.sign_in = None;
            let body = route_authenticated_request(None, &c, None, &self_issued, &q).await;
            assert!(body["sign_in"].is_null(), "{body}");
        }
    }

    /// ADR-119 L1a: the site's path is exactly `/kb/<kb>/<id>`, the id one
    /// percent-decoded segment; nothing else is a site path.
    #[test]
    fn a_site_path_is_one_kb_and_one_decoded_id() {
        use mae_daemon::site::View;
        assert_eq!(
            site_path("/kb/k/n1"),
            Some(("k".into(), View::Page("n1".into())))
        );
        assert_eq!(
            site_path("/kb/k/concept%3Abuffer"),
            Some(("k".into(), View::Page("concept:buffer".into())))
        );
        // The tag index and a tag's page (P-70, P-71); `tags` is reserved.
        assert_eq!(site_path("/kb/k/tags"), Some(("k".into(), View::Tags)));
        // `view` is an ordinary ID: ADR-073's view no longer owns the path
        // (Amendment 2 B4).
        assert_eq!(
            site_path("/kb/k/view"),
            Some(("k".into(), View::Page("view".into())))
        );
        // The KB's home (Amendment 2 B3): which node is the route's lookup.
        assert_eq!(site_path("/kb/k/"), Some(("k".into(), View::Home(None))));
        assert_eq!(
            site_path("/kb/k/index.json"),
            Some(("k".into(), View::Index))
        );
        assert_eq!(
            site_path("/kb/k/tags/a%23b"),
            Some(("k".into(), View::Tag("a#b".into())))
        );
        for bad in [
            "/kb/k",
            "/kb//",
            "/kb//n",
            "/kb/k/a/b",
            "/kbx/k/n",
            "/x/kb/k/n",
            "/kb/k/%zz",
            "/kb/k/%ff",
            "/kb/k/%+f",
            "/kb/k/%-1",
            "/kb/k/tags/",
            "/kb/k/tags/a/b",
            "/kb/k/other/a",
        ] {
            assert_eq!(site_path(bad), None, "{bad}");
        }
        // Every path under /kb/ is the site's: a malformed one is its
        // not-found (`Some(None)`), not the listener's bearer probe.
        for kb_path in ["/kb/k", "/kb/k/a/b", "/kb//n", "/kb/k/tags/"] {
            assert_eq!(site_request(kb_path), Some(None), "{kb_path}");
        }
        assert_eq!(
            site_request("/kb/k/n1"),
            Some(Some(("k".into(), View::Page("n1".into()))))
        );
        for other in ["/", "/kbx/k/n", "/x/kb/k/n", "/mcp"] {
            assert_eq!(site_request(other), None, "{other}");
        }
    }

    /// Every site response carries the CSP and the no-sniff, no-referrer,
    /// no-store headers, a refusal included.
    #[test]
    fn site_responses_carry_their_security_headers() {
        for status in [
            StatusCode::OK,
            StatusCode::NOT_FOUND,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            let r = site_response(status, "x".into());
            let h = r.headers();
            assert_eq!(h["content-security-policy"], mae_daemon::site::CSP);
            assert_eq!(h["x-content-type-options"], "nosniff");
            assert_eq!(h["referrer-policy"], "no-referrer");
            assert_eq!(h["cache-control"], "private, no-store");
        }
        assert!(mae_daemon::site::CSP.contains("script-src 'self'"));
        assert!(!mae_daemon::site::CSP.contains("unsafe-inline"));
    }

    /// A refusal is the same response whatever the site's configuration (A5):
    /// a configured site title would otherwise tell a stranger which KBs exist.
    /// A readable page does carry it (the positive control).
    #[tokio::test]
    async fn a_refusal_never_carries_the_site_title() {
        use http_body_util::BodyExt;
        use mae_daemon::site::{Chrome, SitePage};
        let titled = Chrome {
            lang: "en-US",
            site: Some(("A secret KB name", "/kb/secret/")),
        };
        async fn parts(r: Response<Full<Bytes>>) -> (StatusCode, String, String) {
            let status = r.status();
            let headers = format!("{:?}", r.headers());
            let body = r.into_body().collect().await.unwrap().to_bytes();
            (status, headers, String::from_utf8(body.to_vec()).unwrap())
        }
        for page in [SitePage::NotFound, SitePage::TooLarge] {
            let with = parts(site_answer(page.clone(), titled)).await;
            let without = parts(site_answer(page, "en-US".into())).await;
            assert_eq!(with, without);
            assert!(!with.2.contains("secret"), "{}", with.2);
        }
        let found = SitePage::Found {
            title: "T".into(),
            html: "<p>x</p>".into(),
            backlinks: vec![],
            tags: vec![],
            toc: vec![],
        };
        assert!(parts(site_answer(found, titled))
            .await
            .2
            .contains("A secret KB name"));
    }

    /// The CSP is a contract: the browser client's backend pins it exactly,
    /// per daemon version, and the router's Trusted Types policy must be the
    /// only one allowed (ADR-119 D16). A change here is deliberate and goes to
    /// that backend first.
    #[test]
    fn the_site_csp_is_a_pinned_contract() {
        assert_eq!(
            mae_daemon::site::CSP,
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
             connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; \
             require-trusted-types-for 'script'; trusted-types mae-router"
        );
    }
}
