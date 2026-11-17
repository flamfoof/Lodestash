//! `lodestash-mcp` — Lodestash's MCP server: a thin client of the running app (ADR-0022, v2 spec
//! §7). It serves the protocol catalog's agent operations as MCP tools over stdio and answers each
//! by asking the app over `/v1`, starting the app when none is running. It never opens the catalog.
//!
//! Setup and host registration: `crates/lodestash-mcp/README.md`.

mod forward;
mod launch;
mod tools;

use std::process::ExitCode;

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
    match runtime.block_on(serve()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(%e, "the MCP server stopped");
            ExitCode::FAILURE
        }
    }
}

/// Serve MCP on stdin/stdout until the host closes stdin.
async fn serve() -> std::io::Result<()> {
    use rmcp::ServiceExt;

    let app = forward::App::from_env();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        handshake = ?app.handshake_path(),
        "lodestash-mcp serving on stdio"
    );
    let running = tools::Lodestash::new(app)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(std::io::Error::other)?;
    running.waiting().await.map_err(std::io::Error::other)?;
    Ok(())
}
