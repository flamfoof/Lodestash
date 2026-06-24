//! Lodestash headless CLI — scan, index, tag operations for scripting and CI.

fn main() {
    // Initialize structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lodestash_cli=info".into()),
        )
        .init();

    // TODO: replace with clap command dispatch when implementing CLI commands.
    println!("lodestash-cli skeleton OK");
}
