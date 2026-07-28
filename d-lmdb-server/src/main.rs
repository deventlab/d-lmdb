//! Standalone HTTP server / Docker entrypoint for d-lmdb.

mod http;

use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use clap::Subcommand;
use d_lmdb::DLmdb;
use serde::Deserialize;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the node and serve the HTTP API.
    Serve {
        #[arg(long, env = "CONFIG")]
        config: PathBuf,
    },
    /// Probe this node's /status endpoint; exit 0 if healthy. Used by Docker HEALTHCHECK.
    Healthcheck {
        #[arg(long, env = "CONFIG")]
        config: PathBuf,
    },
}

/// Only the section d-lmdb-server itself cares about — d-lmdb's own `[cluster]`/`[lmdb]`
/// sections in the same file are parsed separately by `DLmdb::open_from_file`.
#[derive(Deserialize)]
struct RootConfig {
    http: HttpSection,
}

#[derive(Deserialize)]
struct HttpSection {
    listen_address: SocketAddr,
}

fn read_http_listen_address(config_path: &Path) -> std::io::Result<SocketAddr> {
    let text = std::fs::read_to_string(config_path)?;
    let root: RootConfig =
        toml::from_str(&text).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(root.http.listen_address)
}

/// Raw HTTP/1.1 GET, no client dependency — this only ever talks to the same
/// container's own loopback address, so no TLS/redirects/DNS are needed.
fn probe_status_endpoint(addr: SocketAddr) -> bool {
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::Duration;

    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = b"GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    if stream.write_all(request).is_err() {
        return false;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }
    response.starts_with("HTTP/1.1 200")
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { config } => {
            let db = DLmdb::open_from_file(&config).await.map_err(std::io::Error::other)?;
            let addr = read_http_listen_address(&config)?;
            http::serve(Arc::new(db), addr).await
        }
        Command::Healthcheck { config } => {
            let addr = read_http_listen_address(&config)?;
            if probe_status_endpoint(addr) {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
    }
}
