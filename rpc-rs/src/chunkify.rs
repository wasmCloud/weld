#![cfg(not(target_arch = "wasm32"))]

// You strike me as a message that has never been chunkified
// I'm sure I don't know what you mean, You forget yourself
// There's a million bytes I haven't sent
// but just you wait, just you wait
//
// Where are you taking me?
// I'm about to change your life
//
// The conversation lasted two seconds, maybe three seconds
// Everything we said in total agreement, it's
// A dream and it's a bit of a dance
// A bit of a posture, it's a bit of a stance
// Each chunk gone by as the bytes advance,
// Jetstream came through, with message enhanced!
//
// I will always be chunkified ...

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use async_nats::jetstream::{
    self,
    object_store::{Config, ObjectStore},
    Context,
};
use once_cell::sync::OnceCell;
use tracing::{debug, error, instrument};

use crate::error::{RpcError, RpcResult};

/// Maximum size of a message payload before it will be chunked
/// Nats currently uses 128kb chunk size so this should be at least 128KB
const CHUNK_THRESHOLD_BYTES: usize = 1024 * 900; // 900KB

/// check if message payload needs to be chunked
pub(crate) fn needs_chunking(payload_size: usize) -> bool {
    payload_size > CHUNK_THRESHOLD_BYTES
}

/// map from lattice to ObjectStore - includes nats client connection
type JsMap = HashMap<String, Context>;

fn jetstream_map() -> Arc<RwLock<JsMap>> {
    static INSTANCE: OnceCell<Arc<RwLock<JsMap>>> = OnceCell::new();
    INSTANCE.get_or_init(|| Arc::new(RwLock::new(JsMap::new()))).clone()
}

pub(crate) fn shutdown() {
    let map = jetstream_map();
    if let Ok(mut w) = map.try_write() {
        w.clear();
    }
    drop(map);
}

#[derive(Clone)]
pub struct ChunkEndpoint {
    lattice: String,
    js: Context,
}

impl ChunkEndpoint {
    pub fn new(lattice: String, js: Context) -> Self {
        ChunkEndpoint { lattice, js }
    }

    /// load the message after de-chunking
    #[instrument(level = "trace", skip(self))]
    pub async fn get_unchunkified(&self, inv_id: &str) -> RpcResult<Vec<u8>> {
        use futures::TryFutureExt as _;
        use tokio::io::AsyncReadExt as _;
        let mut result = Vec::new();
        let store = self.create_or_reuse_store().await?;
        debug!(invocation_id = %inv_id, "chunkify starting to receive");
        let mut obj = store.get(inv_id).await.map_err(|e| {
            RpcError::Nats(format!(
                "error starting to receive chunked stream for inv {}:{}",
                inv_id, e
            ))
        })?;
        let r = obj.read_to_end(&mut result);
        r.map_err(|e| {
            RpcError::Nats(format!(
                "error receiving chunked stream for inv {}:{}",
                inv_id, e
            ))
        })
        .await?;
        if let Err(e) = store.delete(inv_id).await {
            // not deleting will be a non-fatal error for the receiver,
            // if all the bytes have been received
            error!(invocation_id = %inv_id, error = %e, "deleting chunks for inv");
        }
        Ok(result)
    }

    /// load response after de-chunking
    pub async fn get_unchunkified_response(&self, inv_id: &str) -> RpcResult<Vec<u8>> {
        // responses are stored in the object store with '-r' suffix on the object name
        self.get_unchunkified(&format!("{}-r", inv_id)).await
    }

    /// chunkify a message
    #[instrument(level = "trace", skip(self, bytes))]
    pub async fn chunkify(
        &self,
        inv_id: &str,
        mut bytes: (impl tokio::io::AsyncRead + std::marker::Unpin),
    ) -> RpcResult<()> {
        let store = self.create_or_reuse_store().await?;
        debug!(invocation_id = %inv_id, "chunkify starting to send");
        let info = store
            .put(inv_id, &mut bytes)
            .await
            .map_err(|e| RpcError::Nats(format!("writing chunkified for {}: {}", inv_id, e)))?;
        debug!(?info, invocation_id = %inv_id, "chunkify completed writing");

        Ok(())
    }

    /// chunkify a portion of a response
    pub async fn chunkify_response(
        &self,
        inv_id: &str,
        bytes: &mut (impl tokio::io::AsyncRead + std::marker::Unpin),
    ) -> Result<(), RpcError> {
        self.chunkify(&format!("{}-r", inv_id), bytes).await
    }

    async fn create_or_reuse_store(&self) -> RpcResult<ObjectStore> {
        let store = match self.js.get_object_store(&self.lattice).await {
            Ok(store) => store,
            Err(_) => self
                .js
                .create_object_store(Config {
                    bucket: self.lattice.clone(),
                    ..Default::default()
                })
                .await
                .map_err(|e| RpcError::Nats(format!("Failed to create store: {}", &e)))?,
        };
        Ok(store)
    }
}

pub(crate) fn chunkify_endpoint(
    domain: Option<String>,
    nc: async_nats::Client,
    lattice: String,
) -> RpcResult<ChunkEndpoint> {
    let js = connect_js(domain, nc, &lattice)?;
    Ok(ChunkEndpoint::new(lattice, js))
}

pub(crate) fn connect_js(
    domain: Option<String>,
    nc: async_nats::Client,
    lattice_prefix: &str,
) -> RpcResult<Context> {
    let map = jetstream_map();
    let mut _w = map.write().unwrap(); // panics if lock is poisioned
    let js: Context = if let Some(js) = _w.get(lattice_prefix) {
        js.clone()
    } else {
        let js = if let Some(domain) = domain {
            jetstream::with_domain(nc, domain)
        } else {
            jetstream::new(nc)
        };
        _w.insert(lattice_prefix.to_string(), js.clone());
        js
    };
    Ok(js)
}
