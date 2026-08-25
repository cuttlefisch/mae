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
    /// It serves bearer-authenticated KB content, and the webview response
    /// embeds a live access token in its HTML, so an absent `no-store` lets a
    /// browser persist that token to its on-disk cache and lets an
    /// intermediary retain authenticated KB content. RFC 6749 §5.1 (carried
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
            jwks_url: "https://unused-in-these-tests.example.com/jwks".to_string(),
            issuer: Some("https://idp.example.com".to_string()),
            kb_query_enabled: false,
            max_request_body_bytes: 1_048_576,
            kb_query_max_body_bytes: 65_536,
            kb_query_max_scan_nodes: 500,
            kb_query_max_search_results: 20,
            webview_enabled: false,
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
        assert_eq!(principal.principal, "alice@example.com");
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
}
