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

## SignalR: the query-string fallback, and refresh on a live connection

<https://learn.microsoft.com/en-us/aspnet/core/signalr/authn-and-authz>: "browsers can't set custom
headers, such as `Authorization`, on the WebSocket and Server-Sent Events APIs. Because of this
browser limitation, SignalR sends the token as a query string parameter for those transports when
the client runs in a browser." It also warns that "many servers log query string values".
Non-browser clients send the header.

With `EnableAuthenticationRefresh`, a client refreshes the identity of an open connection, and
`CloseOnAuthenticationExpiration` closes connections whose tokens expire. The built-in check
"compares the first `sub`, `ClaimTypes.NameIdentifier`, or `ClaimTypes.Upn` claim in each
principal". A refresh is rejected when the refreshed principal has "a different value, issuer, or
claim type".

**Decides:**
- the native-header / browser-fallback split;
- close on expiry, with refresh;
- a refresh must keep both issuer and subject (Amendment 1 B3).

## Heroku: ticket-based WebSocket authentication

<https://devcenter.heroku.com/articles/websocket-security>: "One pattern we've seen that seems to
solve the WebSocket authentication problem well is a 'ticket'-based authentication system." The
client obtains a ticket from the HTTP server, and the server stores it and later checks that it
"hasn't been re-used and hasn't expired". The ticket "typically contains … the IP of the client
requesting the ticket". The page gives no expiry duration or format.

**Decides:** the ticket shape. The IP binding is deliberately not taken, because behind a
TLS-terminating proxy every client has the proxy's address.

## Ably: refresh over the open connection

<https://ably.com/docs/auth/token>: the SDK "Requests a new token before the current token expires"
and "Maintains the connection seamlessly during refresh". On `authorize()`, "The new token is sent
to Ably using the existing realtime connection", and "The updated permissions are automatically
applied without disconnecting."

**Decides:** renewal travels in-band on the same socket (B3), rather than over a side endpoint as
SignalR does.

## Yjs: y-protocols and Hocuspocus (for B0, D2 re-evaluated)

- <https://gh.ali360.cloudns.ch/yjs/y-protocols/blob/master/PROTOCOL.md>:
  - sync messages are `varUint(0) • varBuffer(stateVector)` (SyncStep1),
    `varUint(1) • varBuffer(documentUpdate)` (SyncStep2) and `varUint(2) • varBuffer(documentUpdate)`
    (Update);
  - SyncStep2 is computed from the responder's own document;
  - the sync section says nothing about authenticating or signing updates;
  - awareness is unauthenticated, so a peer can claim arbitrary presence, and identity must be
    enforced at a higher layer.
- <https://gh.ali360.cloudns.ch/ueberdosis/hocuspocus/releases/tag/v2.0.0>: "Multiplexing allows you to
  synchronize multiple documents over the same websocket connection." The message layout changed
  from `[HocuspocusMessageType][YjsMessageType][yjs binary]` to
  `[documentName][HocuspocusMessageType][YjsMessageType][yjs binary]`.
- <https://tiptap.dev/docs/hocuspocus/server/hooks>:
  - `onAuthenticate` receives a `token` and the `documentName`;
  - `onTokenSync` lets the server "validate user tokens during active sessions without requiring a
    full reconnection";
  - `connection.readOnly` makes a connection read-only;
  - throwing in `beforeHandleMessage` closes the connection;
  - `onStateless` carries string payloads;
  - the hooks hand over the server's `Y.Doc`.
- <https://docs.rs/crate/hocuspocus-rs/0.1.2>: a Rust implementation exists, at 0.1.x.

**Decides:** D2 holds, but on necessity. An update carries no metadata, and the server merges and
answers from its own plaintext document. So MAE's per-update signed authorship, relayed for peers
to re-verify, and its E2E op-sets have no place in the standard protocol. The standard is still
adopted where it fits: Yjs payload encodings, and y-protocols awareness with relative-position
cursors for presence.

## Already recorded elsewhere

ADR-111's research note records that Hocuspocus and Liveblocks leave the token-expiry gap on open
connections, which is why ADR-111 D6 requires an in-band refresh deadline. For Hocuspocus that is
now out of date: `onTokenSync` (above) refreshes a token in session. ADR-111 D6 carries a
correction, and the decision is unchanged. Hocuspocus's
authentication guide (<https://tiptap.dev/docs/hocuspocus/guides/authentication>) shows a `token`
provider option. It does not say how the token is transported or whether it is re-checked on
expiry.
