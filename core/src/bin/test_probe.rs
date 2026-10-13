use anyhow::Result;
use dm_core::network::client::HttpClient;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let client = HttpClient::new();
    let url = "https://speed.hetzner.de/100MB.bin";
    match client.probe(url, None, None, None).await {
        Ok(info) => println!("Success: {:?}", info),
        Err(e) => println!("Error: {:?}", e),
    }
    Ok(())
}
