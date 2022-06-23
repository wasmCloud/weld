use anyhow::{anyhow, Result};
use clap::Parser;
use futures::StreamExt;
use nkeys::KeyPairType;
use std::sync::Arc;
use wascap::prelude::KeyPair;
use wasmbus_rpc::rpc_client::RpcClient;

/// RpcClient test CLI for connection and subscription
#[derive(Parser)]
#[clap(version, about, long_about = None)]
struct Args {
    /// Nats uri. Defaults to 'nats://127.0.0.1:4222'
    #[clap(short, long)]
    nats: Option<String>,

    /// Subject (topic)
    #[clap(value_parser)]
    subject: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // To enable otel tracing, uncomment the line below.
    //wasmbus_rpc::tracing::init_otel_tracing("sub_bin", true);
    let args = Args::parse();
    if args.subject.is_empty() {
        return Err(anyhow!("subject may not be empty"));
    }
    let kp = Arc::new(KeyPair::new(KeyPairType::User));
    let nats_uri = args.nats.unwrap_or_else(|| "nats://127.0.0.1:4222".to_string());
    let nc = async_nats::connect(&nats_uri).await?;
    let client = RpcClient::new(nc, "HOST".into(), None, kp);

    println!("Subscribing to {}", &args.subject);

    let mut sub = client.client().subscribe(args.subject).await?;
    while let Some(msg) = sub.next().await {
        println!("{}", String::from_utf8_lossy(&msg.payload));
    }
    Ok(())
}
