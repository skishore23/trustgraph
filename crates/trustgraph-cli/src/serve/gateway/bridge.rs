//! `trustgraph gateway stdio`: for an MCP client that can only launch a
//! command. It relays JSON-RPC messages between its stdin/stdout and the
//! gateway's streamable HTTP endpoint, with the agent's bearer token, and
//! adds nothing else: no tools, no state, no decisions.

use std::path::Path;

use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, Transport};
use rmcp::{RoleClient, RoleServer};

/// Read the agent's token file: exactly one 64-character hex token.
fn read_token(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let token = text.trim();
    let ok = token.len() == 64
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !ok {
        return Err(format!("{} does not hold an agent token", path.display()));
    }
    Ok(token.to_string())
}

/// Relay until either side closes. Returns the exit code.
pub async fn run(url: &str, token_file: &Path) -> i32 {
    let token = match read_token(token_file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("trustgraph gateway stdio: {e}");
            return 2;
        }
    };
    let config = StreamableHttpClientTransportConfig::with_uri(url).auth_header(token);
    let mut http: StreamableHttpClientTransport<_> =
        StreamableHttpClientTransport::from_config(config);
    let mut stdio: AsyncRwTransport<RoleServer, _, _> =
        AsyncRwTransport::new_server(tokio::io::stdin(), tokio::io::stdout());
    loop {
        tokio::select! {
            from_client = stdio.receive() => {
                let Some(message) = from_client else { break };
                if let Err(e) = Transport::<RoleClient>::send(&mut http, message).await {
                    eprintln!("trustgraph gateway stdio: cannot reach the gateway: {e}");
                    break;
                }
            }
            from_gateway = Transport::<RoleClient>::receive(&mut http) => {
                let Some(message) = from_gateway else {
                    eprintln!("trustgraph gateway stdio: the gateway closed the connection");
                    break;
                };
                if stdio.send(message).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = Transport::<RoleClient>::close(&mut http).await;
    0
}
