//! `lodestash-mcp` — Lodestash's MCP server: a thin client of the running app (ADR-0022, v2 spec
//! §7). It serves the protocol catalog's agent operations as MCP tools over stdio and answers each
//! by asking the app over `/v1`, starting the app when none is running. It never opens the catalog.
//!
//! Setup and host registration: `crates/lodestash-mcp/README.md`.

mod forward;
mod launch;
mod tools;

use std::pin::Pin;
use std::process::ExitCode;
use std::task::{Context, Poll};
use std::time::Duration;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        // stderr, always: stdout is the JSON-RPC channel, and one log line there is a malformed
        // message to the host.
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lodestash_mcp=info".into()),
        )
        .init();
    #[cfg(windows)]
    launch::keep_stdio_out_of_children();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!(%e, "no async runtime");
            return ExitCode::FAILURE;
        }
    };
    let served = runtime.block_on(serve());
    // The host is gone. Dropping the runtime would wait for every call still in flight on the
    // blocking pool — up to a call's whole timeout — to answer a host that will never read it.
    // Leave now; a request already sent is the app's to finish.
    runtime.shutdown_background();
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(%e, "the MCP server stopped");
            ExitCode::FAILURE
        }
    }
}

/// How long the server stays after the host closes stdin: long enough for an answer already on
/// its way to be written, short enough that a host waiting for this process to exit is not kept
/// waiting. (Left to itself the MCP runtime waits five seconds for calls in flight.)
const LEAVE_AFTER_STDIN_CLOSES: Duration = Duration::from_secs(1);

/// stdin, noticing when the host closes it.
struct HostInput {
    stdin: tokio::io::Stdin,
    closed: tokio::sync::watch::Sender<bool>,
}

impl tokio::io::AsyncRead for HostInput {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let wanted = buf.remaining() > 0;
        let before = buf.filled().len();
        let read = Pin::new(&mut self.stdin).poll_read(cx, buf);
        // A read that asked for bytes and completed with none is the end of input; a failed one
        // is as final.
        let ended = match &read {
            Poll::Ready(Ok(())) => wanted && buf.filled().len() == before,
            Poll::Ready(Err(_)) => true,
            Poll::Pending => false,
        };
        if ended {
            let _ = self.closed.send(true);
        }
        read
    }
}

/// Serve MCP on stdin/stdout until the host closes stdin, and leave promptly when it does — even
/// with a call still waiting on the app.
async fn serve() -> std::io::Result<()> {
    use rmcp::ServiceExt;

    let app = forward::App::from_env();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        handshake = ?app.handshake_path(),
        "lodestash-mcp serving on stdio"
    );
    let (closed, mut host_closed) = tokio::sync::watch::channel(false);
    let input = HostInput {
        stdin: tokio::io::stdin(),
        closed,
    };
    let running = tools::Lodestash::new(app)
        .serve((input, tokio::io::stdout()))
        .await
        .map_err(std::io::Error::other)?;
    let host_left = async move {
        // An error means the reader itself is gone — the input with it.
        let _ = host_closed.wait_for(|closed| *closed).await;
        tokio::time::sleep(LEAVE_AFTER_STDIN_CLOSES).await;
    };
    tokio::select! {
        quit = running.waiting() => quit.map(|_| ()).map_err(std::io::Error::other),
        () = host_left => Ok(()),
    }
}

/// Test stand-ins shared with the integration tests: fake app binaries and a raw listener.
#[cfg(test)]
mod testkit {
    #[path = "../../tests/common/fake_app.rs"]
    pub mod fake_app;
    #[path = "../../tests/common/raw.rs"]
    pub mod raw;
}
