//! ADR-066 Phase A: cross-platform local IPC for the editor's own MCP socket
//! (explicitly NOT `mae-daemon`'s socket layer — the daemon stays Unix-only per
//! Gate W's own scoping, ADR-057/066).
//!
//! A real Unix domain socket on Linux/macOS (unchanged — every existing code path's
//! behavior is provably identical by construction, since the `#[cfg(unix)]` arms below
//! are the same calls that were made directly before this module existed) and a
//! Windows named pipe on Windows, behind one shared interface — CLAUDE.md principle #8:
//! one implementation dispatching per platform, not a Windows-specific fork drifting
//! from the Unix path over time.
//!
//! **Windows named pipes live in a namespace with no relation to the filesystem**
//! (`\\.\pipe\<name>`, not a path on any mounted volume, no directory hierarchy, no
//! POSIX file-permission model) — [`pipe_name_for`] derives a stable,
//! collision-resistant pipe name from the SAME logical path every existing call site
//! already constructs (`/tmp/mae-{pid}.sock`, ADR-055's stable per-project path, …), so
//! no call site needs to change how it names its own socket — only the bind/accept/
//! connect primitives become platform-aware.
//!
//! **Not locally verified against a real Windows target in this change** (no Windows
//! toolchain available in the environment this was authored in) — the Unix path is
//! unchanged and fully covered by the existing test suite; the Windows path is written
//! against tokio's documented `named_pipe` API and is verified by ADR-066 Phase C's new
//! `windows-latest` CI leg, iterated against real CI feedback rather than claimed
//! correct on the strength of code review alone.

use std::io;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Derive a stable Windows named-pipe name from a logical socket path. Pure and
/// deterministic (the same path always yields the same pipe name across a bind and a
/// later connect attempt against it — required for `headless_loop.rs`'s stable-socket
/// discovery to work at all), collision-resistant (SHA-256, not a weak/short hash), and
/// sidesteps every Windows pipe-name restriction a raw path could violate (no `:`, no
/// `\`, a bounded length) since the whole path is hashed rather than embedded literally.
pub fn pipe_name_for(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    format!(r"\\.\pipe\mae-{}", hex::encode(&digest[..16]))
}

/// A connected local-IPC stream — a Unix domain socket, or (on Windows) either side of
/// a named-pipe connection. `AsyncRead`/`AsyncWrite` are implemented identically across
/// variants so every caller of the underlying byte stream (session/framing logic in
/// `lib.rs`, already generic over `AsyncRead`/`AsyncWrite` via `read_message`/
/// `write_framed`) needs zero changes regardless of platform or which side of the
/// connection this is.
#[derive(Debug)]
pub enum LocalStream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    #[cfg(windows)]
    NamedPipeServer(tokio::net::windows::named_pipe::NamedPipeServer),
    #[cfg(windows)]
    NamedPipeClient(tokio::net::windows::named_pipe::NamedPipeClient),
}

impl AsyncRead for LocalStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            LocalStream::Unix(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            LocalStream::NamedPipeServer(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(windows)]
            LocalStream::NamedPipeClient(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for LocalStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(unix)]
            LocalStream::Unix(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            LocalStream::NamedPipeServer(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(windows)]
            LocalStream::NamedPipeClient(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            LocalStream::Unix(s) => Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            LocalStream::NamedPipeServer(s) => Pin::new(s).poll_flush(cx),
            #[cfg(windows)]
            LocalStream::NamedPipeClient(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            LocalStream::Unix(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            LocalStream::NamedPipeServer(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(windows)]
            LocalStream::NamedPipeClient(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// The listening side of a local-IPC address. `accept()` takes `&mut self` because the
/// Windows named-pipe implementation must hold a single pending (not-yet-connected)
/// pipe instance between calls and replace it with a fresh one on every accepted
/// connection (tokio's documented multi-client named-pipe server pattern — a named pipe
/// instance is one connection slot, not a reusable listener the way a Unix socket fd
/// is) — `&self` would not be sufficient to model that state transition. The Unix path
/// doesn't need this (`UnixListener::accept` already takes `&self`) but takes `&mut
/// self` here too so callers have one uniform signature regardless of platform.
pub struct LocalListener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(windows)]
    pipe_name: String,
    #[cfg(windows)]
    pending: tokio::net::windows::named_pipe::NamedPipeServer,
}

impl LocalListener {
    /// Bind the local-IPC address derived from `path`. On Unix, `path` is used exactly
    /// as before (a real socket file at that path — unchanged behavior). On Windows,
    /// `path` is translated via [`pipe_name_for`] into the actual named-pipe name; no
    /// file is created on disk.
    pub fn bind(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let inner = tokio::net::UnixListener::bind(path)?;
            Ok(Self { inner })
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            let pipe_name = pipe_name_for(path);
            let pending = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe_name)?;
            Ok(Self { pipe_name, pending })
        }
    }

    /// Accept the next incoming connection. Supports multiple concurrent clients on
    /// both platforms — see the struct doc comment for why Windows needs `&mut self`
    /// to do this correctly (create the next pending instance before handing the
    /// connected one off, so a client racing in between two `accept()` calls is never
    /// silently dropped).
    pub async fn accept(&mut self) -> io::Result<LocalStream> {
        #[cfg(unix)]
        {
            let (stream, _addr) = self.inner.accept().await?;
            Ok(LocalStream::Unix(stream))
        }
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ServerOptions;
            self.pending.connect().await?;
            // Swap in a fresh pending instance BEFORE returning the connected one, so
            // a client connecting between this line and the next accept() call still
            // finds a listener present rather than a window with none.
            let next = ServerOptions::new().create(&self.pipe_name)?;
            let connected = std::mem::replace(&mut self.pending, next);
            Ok(LocalStream::NamedPipeServer(connected))
        }
    }
}

/// The connecting (client) side of a local-IPC address — used by `mae-mcp-shim` and
/// `headless_loop.rs`'s stable-socket live-listener probe. On Unix, identical to a
/// direct `UnixStream::connect` call. On Windows, opens the named pipe derived from
/// `path` via [`pipe_name_for`] — the SAME derivation [`LocalListener::bind`] uses, so a
/// client connecting to a given logical path always reaches the server that bound that
/// same path, never a different pipe by accident.
pub async fn connect(path: &Path) -> io::Result<LocalStream> {
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(LocalStream::Unix(stream))
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        let pipe_name = pipe_name_for(path);
        // ClientOptions::open is synchronous (opening a named-pipe client handle is a
        // single Win32 call, unlike the server side's async `.connect()` wait) --
        // callers wrapping this in a timeout (e.g. headless_loop.rs's stable-socket
        // probe) still work correctly, the timeout is simply a no-op for the
        // already-resolved case.
        let client = ClientOptions::new().open(&pipe_name)?;
        Ok(LocalStream::NamedPipeClient(client))
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;

    /// `pipe_name_for` is only actually USED on Windows, but its determinism/
    /// collision-resistance properties are pure and platform-independent, so they're
    /// tested on whatever platform this happens to run on (matches this project's
    /// established "pure function, testable regardless of the platform gate around its
    /// call site" pattern — e.g. ADR-063 Phase A/B's helpers).
    #[test]
    fn pipe_name_for_is_deterministic_for_the_same_path() {
        let path = Path::new("/tmp/mae-1234.sock");
        assert_eq!(pipe_name_for(path), pipe_name_for(path));
    }

    #[test]
    fn pipe_name_for_differs_for_different_paths() {
        let a = pipe_name_for(Path::new("/tmp/mae-1234.sock"));
        let b = pipe_name_for(Path::new("/tmp/mae-5678.sock"));
        assert_ne!(
            a, b,
            "distinct logical paths must not collide onto the same pipe name"
        );
    }

    #[test]
    fn pipe_name_for_has_the_correct_windows_namespace_prefix() {
        let name = pipe_name_for(Path::new("/tmp/mae-1234.sock"));
        assert!(name.starts_with(r"\\.\pipe\mae-"));
    }

    #[test]
    fn pipe_name_for_never_embeds_characters_a_real_path_could_contain() {
        // A real path can contain ':', '\', spaces, unicode -- none of that may leak
        // into the derived pipe name (Windows pipe names have their own restrictions);
        // the hash-based derivation should make this true by construction, verified
        // directly rather than assumed.
        let name = pipe_name_for(Path::new(r"C:\Users\weird name\mae-1234.sock"));
        let suffix = name.strip_prefix(r"\\.\pipe\mae-").unwrap();
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "expected a pure hex digest suffix, got: {suffix}"
        );
    }

    #[tokio::test]
    async fn local_listener_accept_and_connect_round_trip_on_unix() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.sock");
        let mut listener = LocalListener::bind(&path).expect("bind");

        let accept_task = tokio::spawn(async move { listener.accept().await });
        // Give the accept loop a moment to actually be polling.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let client = connect(&path).await.expect("connect");
        let server_stream = accept_task.await.unwrap().expect("accept");

        // Both ends must be genuinely connected to EACH OTHER, not just independently
        // constructible -- write on one side, read on the other, over the real socket.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut client = client;
        let mut server_stream = server_stream;
        client.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        server_stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }
}
