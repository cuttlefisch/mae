# ADR-099 Amendment 1 prior art: authenticating a browser WebSocket

Date: 2026-11-11. Each item records what was checked and what it decides. Where a source did not
say something, that is stated rather than inferred.

## The browser API cannot set headers

<https://websockets.spec.whatwg.org/> defines the constructor as
`constructor(USVString url, optional (DOMString or sequence<DOMString>) protocols = [])`. The
handshake headers are fixed by the algorithm: `Upgrade`, `Connection`, `Sec-WebSocket-Key`,
`Sec-WebSocket-Version`, `Sec-WebSocket-Protocol` and `Sec-WebSocket-Extensions`. A script cannot
add `Authorization`. The handshake runs through Fetch with credentials mode `include`, so cookies are
sent. On `protocols`: "The connection will only be established if the server reports that it has
selected one of these subprotocols."

**Decides:** ADR-099 D3's sentence "a WebSocket client is script-driven and can set headers" is
wrong for browsers. A browser has three places for a credential: the URL, the subprotocol list, or a
frame after the connection opens. Native clients can still send `Authorization`.

## OWASP WebSocket Security Cheat Sheet

<https://cheatsheetseries.owasp.org/cheatsheets/WebSocket_Security_Cheat_Sheet.html>:
- "Avoid tokens in URL query strings because they can leak into access logs."
- "prefer sending the token in the first message over WSS", and "Until token validation succeeds,
  accept only the authentication message and do not send protected data."
- "Browsers include cookies in WebSocket handshake requests", which is what makes cross-site
  WebSocket hijacking possible. "Validate the `Origin` header on every handshake", with "an explicit
  allowlist of trusted origins", avoiding "wildcards or substring matching".
- On long-lived connections: "Close WebSocket connections when sessions expire", and "Rotate tokens
  in long-lived connections to prevent hijacked sessions from persisting."
- Message sizes: "Set reasonable size limits (typically 64KB or less)".

The sheet does not cover tickets or the subprotocol header.

**Decides:**
- no credential in the URL;
- an exact-match Origin allowlist;
- the server closes a connection whose credential expires, and renews it in-band;
- explicit frame-size limits.

First-message authentication is the sheet's preference. It is not chosen here (see Kubernetes,
below, and the amendment's rejected alternatives): it opens an unauthenticated socket, and that
moves the authentication decision off the listener's existing, tested rejection path.

## Kubernetes: the bearer token as a subprotocol

<https://pkg.go.dev/k8s.io/apiserver/pkg/authentication/request/websocket>: `ProtocolAuthenticator`
"allows a websocket connection to provide a bearer token as a subprotocol in the format
`base64url.bearer.authorization.<base64url-without-padding(bearer-token)>`". The package
documentation does not say whether the token subprotocol is removed from the negotiated set.

A Kubernetes Python client issue (kubernetes-client/python#2595) reports a 401 after the client
began offering more than one subprotocol, because the server took the subprotocol auth path and did
not fall back. That is a reminder that the auth entry and the version entry must be told apart
deterministically.

**Decides:** the subprotocol list is a credential carrier that browsers can set, that is
authenticated before the upgrade completes, and that is not part of the URL. The server never
echoes a credential-bearing entry, only the version entry. It also does not carry the long-lived
bearer itself, only a short single-use ticket, because the header still passes through the proxy.

## Slack: a short-lived connect URL

<https://docs.slack.dev/reference/methods/rtm.connect>: the method returns a WebSocket URL, and
"These URLs are only valid for 30 seconds, so connect quickly!" The page does not say whether a URL
is single-use. The `rtm.connect` call itself is authenticated with the normal token, in a header or
a POST parameter.

**Decides:** the two-step shape. An authenticated HTTPS call mints a short-lived connect credential,
and the WebSocket presents it. MAE tightens this to single-use and keeps it out of the URL.

## Already recorded elsewhere

ADR-111's research note records that Hocuspocus and Liveblocks leave the token-expiry gap on open
connections, which is why ADR-111 D6 requires an in-band refresh deadline. Hocuspocus's
authentication guide (<https://tiptap.dev/docs/hocuspocus/guides/authentication>) shows a `token`
provider option. It does not say how the token is transported or whether it is re-checked on
expiry.
