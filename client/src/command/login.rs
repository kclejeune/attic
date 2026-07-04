use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use clap::Parser;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::cache::ServerName;
use crate::cli::Opts;
use crate::config::{Config, ServerConfig, ServerTokenConfig};

/// Log into an Attic server.
#[derive(Debug, Parser)]
pub struct Login {
    /// Name of the server.
    name: ServerName,

    /// Endpoint of the server.
    endpoint: String,

    /// Access token.
    ///
    /// Omit this and pass --web or --device to obtain one interactively.
    token: Option<String>,

    /// Set the server as the default.
    #[clap(long)]
    set_default: bool,

    /// Authenticate in a browser (loopback OAuth), obtaining a token automatically.
    #[clap(long, conflicts_with = "device")]
    web: bool,

    /// Authenticate with a device code (for headless machines).
    #[clap(long, conflicts_with = "web")]
    device: bool,
}

pub async fn run(opts: Opts) -> Result<()> {
    let sub = opts.command.as_login().unwrap();

    let token = if sub.web {
        Some(web_login(&sub.endpoint).await?)
    } else if sub.device {
        Some(device_login(&sub.endpoint).await?)
    } else {
        sub.token.clone()
    };

    let mut config = Config::load()?;
    let mut config_m = config.as_mut();

    if let Some(server) = config_m.servers.get_mut(&sub.name) {
        eprintln!("✍️ Overwriting server \"{}\"", sub.name.as_str());
        server.endpoint = sub.endpoint.to_owned();
        if let Some(token) = &token {
            server.token = Some(ServerTokenConfig::Raw {
                token: token.clone(),
            });
        }
    } else {
        eprintln!("✍️ Configuring server \"{}\"", sub.name.as_str());
        config_m.servers.insert(
            sub.name.to_owned(),
            ServerConfig {
                endpoint: sub.endpoint.to_owned(),
                token: token.clone().map(|token| ServerTokenConfig::Raw { token }),
            },
        );
    }

    if sub.set_default || config_m.servers.len() == 1 {
        config_m.default_server = Some(sub.name.to_owned());
    }

    if token.is_some() {
        eprintln!("✅ Logged in to \"{}\"", sub.name.as_str());
    }

    Ok(())
}

#[derive(Deserialize)]
struct AuthConfig {
    authorize_url: Option<String>,
    device_authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct DeviceStart {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    interval: u64,
    expires_in: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    token: Option<String>,
    error: Option<String>,
}

async fn discover(endpoint: &str) -> Result<AuthConfig> {
    let base = endpoint.trim_end_matches('/');
    reqwest::Client::new()
        .get(format!("{base}/_api/v1/auth-config"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .map_err(|e| anyhow!("Failed to read auth config: {e}"))
}

/// Browser loopback flow: open the admin app, capture the token on 127.0.0.1.
async fn web_login(endpoint: &str) -> Result<String> {
    let cfg = discover(endpoint).await?;
    let authorize = cfg
        .authorize_url
        .ok_or_else(|| anyhow!("This server does not advertise a browser login URL"))?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let state = random_hex(16);
    let hostname = gethostname::gethostname().to_string_lossy().to_string();

    let mut url = reqwest::Url::parse(&authorize)?;
    url.query_pairs_mut()
        .append_pair("port", &port.to_string())
        .append_pair("state", &state)
        .append_pair("hostname", &hostname)
        .append_pair("label", &format!("attic CLI ({hostname})"));
    let url = url.to_string();

    eprintln!("Opening your browser to authorize…");
    eprintln!("If it doesn't open, visit:\n  {url}\n");
    let _ = webbrowser::open(&url);

    wait_for_callback(listener, &state).await
}

async fn wait_for_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String> {
    let deadline = Duration::from_secs(300);
    loop {
        let (mut stream, _) = tokio::time::timeout(deadline, listener.accept())
            .await
            .map_err(|_| anyhow!("Timed out waiting for browser authorization"))??;

        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("");

        // Parse the query through a URL so values are percent-decoded.
        if let Ok(parsed) = reqwest::Url::parse(&format!("http://localhost{path}")) {
            let mut token = None;
            let mut state = None;
            for (k, v) in parsed.query_pairs() {
                match k.as_ref() {
                    "token" => token = Some(v.into_owned()),
                    "state" => state = Some(v.into_owned()),
                    _ => {}
                }
            }

            if let Some(token) = token {
                let ok = state.as_deref() == Some(expected_state) && !token.is_empty();
                let body = if ok {
                    "Authorized. You can close this tab and return to your terminal."
                } else {
                    "Authorization failed — the state did not match. Please try again."
                };
                let _ = write_html(&mut stream, body).await;
                return if ok {
                    Ok(token)
                } else {
                    Err(anyhow!("State mismatch in callback"))
                };
            }
        }

        // Not the callback (favicon, etc.) — 404 and keep listening.
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
    }
}

async fn write_html(stream: &mut tokio::net::TcpStream, message: &str) -> Result<()> {
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui;text-align:center;padding:3rem;color:#333\">{message}</body></html>"
    );
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Device-code flow: print a code + URL, poll until the browser approves.
async fn device_login(endpoint: &str) -> Result<String> {
    let base = endpoint.trim_end_matches('/');
    let cfg = discover(endpoint).await?;
    let http = reqwest::Client::new();

    let start: DeviceStart = http
        .post(format!("{base}{}", cfg.device_authorization_endpoint))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    eprintln!(
        "\nTo authorize this device, open:\n  {}\nand enter the code:\n\n    {}\n",
        start.verification_uri, start.user_code
    );
    if let Some(complete) = &start.verification_uri_complete {
        let _ = webbrowser::open(complete);
    }
    eprintln!("Waiting for authorization…");

    let token_url = format!("{base}{}", cfg.token_endpoint);
    let interval = Duration::from_secs(start.interval.max(1));
    let deadline = Instant::now() + Duration::from_secs(start.expires_in);

    loop {
        if Instant::now() > deadline {
            return Err(anyhow!("Device code expired before authorization"));
        }
        tokio::time::sleep(interval).await;

        let resp: TokenResponse = http
            .post(&token_url)
            .json(&serde_json::json!({ "device_code": start.device_code }))
            .send()
            .await?
            .json()
            .await?;

        if let Some(token) = resp.token {
            if !token.is_empty() {
                return Ok(token);
            }
        }
        match resp.error.as_deref() {
            Some("authorization_pending") | None => continue,
            Some("slow_down") => tokio::time::sleep(interval).await,
            Some("access_denied") => return Err(anyhow!("Authorization was denied")),
            Some("expired_token") => return Err(anyhow!("Device code expired")),
            Some(other) => return Err(anyhow!("Authorization failed: {other}")),
        }
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("getrandom");
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}
