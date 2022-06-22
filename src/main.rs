// SPDX-FileCopyrightText: 2022 Profian Inc. <opensource@profian.com>
// SPDX-License-Identifier: AGPL-3.0-only

use std::fs::{read_to_string, File};
use std::io::{self, BufRead, BufReader};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use drawbridge_app::url::Url;
use drawbridge_app::{App, OidcConfig, TlsConfig};

use anyhow::Context as _;
use async_std::net::TcpListener;
use clap::Parser;
use futures::StreamExt;
use log::{debug, error};

/// Server for hosting WebAssembly modules for use in Enarx keeps.
///
/// Any command-line options listed here may be specified by one or
/// more configuration files, which can be used by passing the
/// name of the file on the command-line with the syntax `@my_file`.
/// Each line of the configuration file will be interpreted as one
/// argument to the shell, so keys and values must either be
/// separated by line breaks or by an `=` as in `--foo=bar`.
#[derive(Parser, Debug)]
#[clap(author, version, about)]
struct Args {
    /// Address to bind to.
    ///
    /// If no value is specified for this argument either on
    /// the command line or in a configuration file,
    /// the value will default to the unspecified IPv4 address
    /// 0.0.0.0 and port 8080.
    #[clap(long, default_value_t = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080))]
    addr: SocketAddr,

    /// Path to the Drawbridge store.
    #[clap(long)]
    store: PathBuf,

    /// Path to PEM-encoded server certificate.
    #[clap(long)]
    cert: PathBuf,

    /// Path to PEM-encoded server certificate key.
    #[clap(long)]
    key: PathBuf,

    /// Path to PEM-encoded trusted CA certificate.
    ///
    /// Clients that present a valid certificate signed by this CA
    /// are granted read-only access to all repositories in the store.
    #[clap(long)]
    ca: PathBuf,

    /// OpenID Connect provider label.
    #[clap(long)]
    oidc_label: String,

    /// OpenID Connect issuer URL.
    #[clap(long)]
    oidc_issuer: Url,

    /// OpenID Connect client ID.
    #[clap(long)]
    oidc_client: String,

    /// OpenID Connect secret.
    #[clap(long)]
    oidc_secret: Option<String>,
}

fn open_buffered(p: impl AsRef<Path>) -> io::Result<impl BufRead> {
    File::open(p).map(BufReader::new)
}

#[async_std::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let mut processed_args = Vec::new();
    for arg in std::env::args() {
        match arg.strip_prefix('@') {
            None => processed_args.push(arg),
            Some(path) => {
                let config = read_to_string(path).context("Failed to read config file")?;
                for line in config.lines() {
                    processed_args.push(line.to_string());
                }
            }
        }
    }

    let Args {
        addr,
        store,
        cert,
        key,
        ca,
        oidc_label,
        oidc_issuer,
        oidc_client,
        oidc_secret,
    } = Args::parse_from(processed_args);

    let cert = open_buffered(cert).context("Failed to open server certificate file")?;
    let key = open_buffered(key).context("Failed to open server key file")?;
    let ca = open_buffered(ca).context("Failed to open CA certificate file")?;
    let tls = TlsConfig::read(cert, key, ca).context("Failed to construct server TLS config")?;

    let app = App::new(
        store,
        tls,
        OidcConfig {
            label: oidc_label,
            issuer: oidc_issuer,
            client_id: oidc_client,
            client_secret: oidc_secret,
        },
    )
    .await
    .context("Failed to build app")?;
    TcpListener::bind(addr)
        .await
        .with_context(|| format!("Failed to bind to {}", addr))?
        .incoming()
        .for_each_concurrent(Some(1), |stream| async {
            if let Err(e) = async {
                let stream = stream.context("failed to initialize connection")?;
                debug!(
                    target: "main",
                    "received TCP connection from {}",
                    stream
                        .peer_addr()
                        .map(|peer| peer.to_string())
                        .unwrap_or_else(|_| "unknown address".into())
                );
                app.handle(stream).await
            }
            .await
            {
                error!(target: "main", "failed to handle request: {e}");
            }
        })
        .await;
    Ok(())
}
