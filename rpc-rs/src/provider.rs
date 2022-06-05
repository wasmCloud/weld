#![cfg(not(target_arch = "wasm32"))]

//! common provider wasmbus support
//!
use std::fmt::Formatter;
use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    ops::Deref,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use async_trait::async_trait;
use futures::{future::JoinAll, StreamExt};
use serde::de::DeserializeOwned;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};
use tracing_futures::Instrument;

pub use crate::rpc_client::make_uuid;
use crate::{
    common::{deserialize, serialize, Context, Message, MessageDispatch, SendOpts, Transport},
    core::{
        HealthCheckRequest, HealthCheckResponse, HostData, Invocation, InvocationResponse,
        LinkDefinition,
    },
    error::{RpcError, RpcResult},
    rpc_client::{RpcClient, DEFAULT_RPC_TIMEOUT_MILLIS},
};

// name of nats queue group for rpc subscription
const RPC_SUBSCRIPTION_QUEUE_GROUP: &str = "rpc";

/// nats address to use if not included in initial HostData
pub(crate) const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";

pub type HostShutdownEvent = String;

pub trait ProviderDispatch: MessageDispatch + ProviderHandler {}
trait ProviderImpl: ProviderDispatch + Send + Sync + 'static {}

// temporary holder for invocation info for logging
struct InvLog {
    operation: String,
    origin: String,
    id: String,
    host_id: String,
}
impl InvLog {
    fn init(inv: &Invocation) -> Self {
        InvLog {
            operation: inv.operation.clone(),
            origin: inv.origin.public_key.clone(),
            id: inv.id.clone(),
            host_id: inv.host_id.clone(),
        }
    }
}

pub mod prelude {
    pub use crate::{
        common::{Context, Message, MessageDispatch, SendOpts},
        core::LinkDefinition,
        error::{RpcError, RpcResult},
        provider::{HostBridge, ProviderDispatch, ProviderHandler},
        provider_main::{
            get_host_bridge, load_host_data, provider_main, provider_run, provider_start,
        },
    };

    pub use async_trait::async_trait;
    pub use wasmbus_macros::Provider;

    #[cfg(feature = "BigInteger")]
    pub use num_bigint::BigInt as BigInteger;

    #[cfg(feature = "BigDecimal")]
    pub use bigdecimal::BigDecimal;
}

/// CapabilityProvider handling of messages from host
/// The HostBridge handles most messages and forwards the remainder to this handler
#[async_trait]
pub trait ProviderHandler: Sync {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    /// This message is idempotent - provider must be able to handle
    /// duplicates
    #[allow(unused_variables)]
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        Ok(true)
    }

    /// Notify the provider that the link is dropped
    #[allow(unused_variables)]
    async fn delete_link(&self, actor_id: &str) {}

    /// Perform health check. Called at regular intervals by host
    /// Default implementation always returns healthy
    #[allow(unused_variables)]
    async fn health_request(&self, arg: &HealthCheckRequest) -> RpcResult<HealthCheckResponse> {
        Ok(HealthCheckResponse { healthy: true, message: None })
    }

    /// Handle system shutdown message
    async fn shutdown(&self) -> Result<(), Infallible> {
        Ok(())
    }
}

/// format of log message sent to main thread for output to logger
pub type LogEntry = (tracing::Level, String);

pub type QuitSignal = tokio::sync::broadcast::Receiver<bool>;

#[doc(hidden)]
/// Process subscription, until closed or exhausted, or value is received on the channel.
/// `sub` is a mutable Subscriber (regular or queue subscription)
/// `channel` may be either tokio mpsc::Receiver or broadcast::Receiver, and is considered signaled
/// when a value is sent or the chanel is closed.
/// `msg` is the variable name to be used in the handler
/// `on_item` is an async handler
macro_rules! process_until_quit {
    ($sub:ident, $channel:ident, $msg:ident, $on_item:tt) => {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = $channel.recv() => {
                        let _ = $sub.unsubscribe().await;
                        break;
                    },
                    __msg = $sub.next() => {
                        match __msg {
                            None => break,
                            Some($msg) => {
                                $on_item
                            }
                        }
                    }
                }
            }
        })
    };
}

/// HostBridge manages the NATS connection to the host,
/// and processes subscriptions for links, health-checks, and rpc messages.
/// Callbacks from HostBridge are implemented by the provider in the [[ProviderHandler]] implementation.
///
#[derive(Clone)]
pub struct HostBridge {
    inner: Arc<HostBridgeInner>,
    host_data: HostData,
}

impl HostBridge {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn new_sync_client(&self) -> RpcResult<nats::Connection> {
        let nats_addr = if !self.host_data.lattice_rpc_url.is_empty() {
            self.host_data.lattice_rpc_url.as_str()
        } else {
            DEFAULT_NATS_ADDR
        };
        let nats_opts = match (
            self.host_data.lattice_rpc_user_jwt.trim(),
            self.host_data.lattice_rpc_user_seed.trim(),
        ) {
            ("", "") => nats::Options::default(),
            (rpc_jwt, rpc_seed) => {
                let kp = nkeys::KeyPair::from_seed(rpc_seed).unwrap();
                let jwt = rpc_jwt.to_owned();
                nats::Options::with_jwt(
                    move || Ok(jwt.to_owned()),
                    move |nonce| kp.sign(nonce).unwrap(),
                )
            }
        };
        // Connect to nats
        nats_opts.max_reconnects(None).connect(nats_addr).map_err(|e| {
            RpcError::ProviderInit(format!("nats connection to {} failed: {}", nats_addr, e))
        })
    }

    pub(crate) fn new_client(
        nats: crate::anats::Client,
        host_data: &HostData,
    ) -> RpcResult<HostBridge> {
        let key = if host_data.is_test() {
            wascap::prelude::KeyPair::new_user()
        } else {
            wascap::prelude::KeyPair::from_seed(&host_data.invocation_seed)
                .map_err(|e| RpcError::NotInitialized(format!("key failure: {}", e)))?
        };
        let rpc_client = RpcClient::new_client(
            nats,
            &host_data.lattice_rpc_prefix,
            key,
            host_data.host_id.clone(),
            host_data.default_rpc_timeout_ms.map(|ms| Duration::from_millis(ms as u64)),
        );

        Ok(HostBridge {
            inner: Arc::new(HostBridgeInner {
                links: RwLock::new(HashMap::new()),
                rpc_client,
                lattice_prefix: host_data.lattice_rpc_prefix.clone(),
            }),
            host_data: host_data.clone(),
        })
    }

    /// Returns the provider's public key
    pub fn provider_key(&self) -> &str {
        self.host_data.provider_key.as_str()
    }

    /// Returns the host id that launched this provider
    pub fn host_id(&self) -> &str {
        self.host_data.host_id.as_str()
    }

    /// Returns the link_name for this provider
    pub fn link_name(&self) -> &str {
        self.host_data.link_name.as_str()
    }
}

impl Deref for HostBridge {
    type Target = HostBridgeInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[doc(hidden)]
/// Initialize host bridge for use by wasmbus-test-util.
/// The purpose is so that test code can get the nats configuration
/// This is never called inside a provider process (and will fail if a provider calls it)
pub fn init_host_bridge_for_test(nc: crate::anats::Client, host_data: &HostData) -> RpcResult<()> {
    let hb = HostBridge::new_client(nc, host_data)?;
    crate::provider_main::set_host_bridge(hb)
        .map_err(|_| RpcError::Other("HostBridge already initialized".to_string()))?;
    Ok(())
}

#[doc(hidden)]
pub struct HostBridgeInner {
    /// Table of actors that are bound to this provider
    /// Key is actor_id / actor public key
    links: RwLock<HashMap<String, LinkDefinition>>,
    rpc_client: RpcClient,
    lattice_prefix: String,
}

impl std::fmt::Debug for HostBridge {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostBridge")
            .field("provider_id", &self.host_data.provider_key)
            .field("host_id", &self.host_data.host_id)
            .field("link", &self.host_data.link_name)
            .field("lattice_prefix", &self.lattice_prefix)
            .finish()
    }
}

impl HostBridge {
    /// Returns a reference to the rpc client
    fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }

    // parse incoming subscription message
    // if it fails deserialization, we can't really respond;
    // so log the error
    fn parse_msg<T: DeserializeOwned>(
        &self,
        msg: &crate::anats::Message,
        topic: &str,
    ) -> Option<T> {
        match if self.host_data.is_test() {
            serde_json::from_slice(&msg.payload).map_err(|e| RpcError::Deser(e.to_string()))
        } else {
            deserialize(&msg.payload)
        } {
            Ok(item) => Some(item),
            Err(e) => {
                error!(%topic, error = %e, "garbled data received on topic");
                None
            }
        }
    }

    /// Stores actor with link definition
    pub async fn put_link(&self, ld: LinkDefinition) {
        let mut update = self.links.write().await;
        update.insert(ld.actor_id.to_string(), ld);
    }

    /// Deletes link
    pub async fn delete_link(&self, actor_id: &str) {
        let mut update = self.links.write().await;
        update.remove(actor_id);
    }

    /// Returns true if the actor is linked
    pub async fn is_linked(&self, actor_id: &str) -> bool {
        let read = self.links.read().await;
        read.contains_key(actor_id)
    }

    /// Returns copy of LinkDefinition, or None,if the actor is not linked
    pub async fn get_link(&self, actor_id: &str) -> Option<LinkDefinition> {
        let read = self.links.read().await;
        read.get(actor_id).cloned()
    }

    /// Implement subscriber listener threads and provider callbacks
    pub(crate) async fn connect<P>(
        &'static self,
        provider: P,
        shutdown_tx: &tokio::sync::broadcast::Sender<bool>,
    ) -> JoinAll<tokio::task::JoinHandle<RpcResult<()>>>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        futures::future::join_all(vec![
            tokio::task::spawn(self.subscribe_rpc(provider.clone(), shutdown_tx.subscribe())),
            tokio::task::spawn(self.subscribe_link_put(provider.clone(), shutdown_tx.subscribe())),
            tokio::task::spawn(self.subscribe_link_del(provider.clone(), shutdown_tx.subscribe())),
            tokio::task::spawn(self.subscribe_shutdown(provider.clone(), shutdown_tx.clone())),
            // subscribe to health last, after receivers are set up
            tokio::task::spawn(self.subscribe_health(provider, shutdown_tx.subscribe())),
        ])
    }

    /// flush nats - called before main process exits
    pub(crate) async fn flush(&self) {
        if let Err(error) = self.inner.rpc_client.client().flush().await {
            error!(%error, "flushing nats connection");
        }
    }

    /// Returns the nats rpc topic for capability providers
    pub fn provider_rpc_topic(&self) -> String {
        format!(
            "wasmbus.rpc.{}.{}.{}",
            &self.lattice_prefix, &self.host_data.provider_key, self.host_data.link_name
        )
    }

    /// Subscribe to a nats topic for rpc messages.
    /// This method starts a separate async task and returns immeditely.
    /// It will exit if the nats client disconnects, or if a signal is received on the quit channel.
    pub async fn subscribe_rpc<P>(
        &self,
        provider: P,
        mut quit: tokio::sync::broadcast::Receiver<bool>,
    ) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let mut sub = self
            .rpc_client
            .client()
            .queue_subscribe(
                self.provider_rpc_topic(),
                RPC_SUBSCRIPTION_QUEUE_GROUP.to_string(),
            )
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = quit.recv() => {
                        let _ = sub.unsubscribe().await;
                        break;
                    },
                    nats_msg = sub.next() => {
                        let msg = match nats_msg {
                            None => break,
                            Some(msg) => msg
                        };
                        let inv = match crate::common::deserialize::<Invocation>(&msg.payload) {
                            Ok(inv) => inv,
                            Err(error) => {
                                error!(%error, "invalid rpc message received (not deserializable)");
                                if let Some(reply) = msg.reply {
                                    let _ = this.rpc_client().publish_invocation_response(reply, InvocationResponse{
                                        error: Some(format!("deser error: {}", error)),
                                        ..Default::default()
                                    }).await;
                                }
                                continue;
                            }
                        };
                        let inv_log = InvLog::init(&inv);
                        let resp = match this.handle_rpc(provider.clone(), inv).await {
                            Err(error) => {
                                error!(
                                    operation = %inv_log.operation,
                                    public_key = %inv_log.origin,
                                    invocation_id = %inv_log.id,
                                    host_id = %inv_log.host_id,
                                    %error,
                                    "Invocation failed"
                                );
                                InvocationResponse{
                                    invocation_id: inv_log.id,
                                    error: Some(error.to_string()),
                                    ..Default::default()
                                }
                            },
                            Ok(bytes) => {
                                InvocationResponse{
                                    invocation_id: inv_log.id,
                                    content_length: Some(bytes.len() as u64),
                                    msg: bytes,
                                    ..Default::default()
                                }
                            }
                        };
                        if let Some(reply) = msg.reply {
                            // send reply
                            let _ = this.rpc_client().publish_invocation_response(reply, resp).await;
                        }
                    }
                }
            }
        });
        Ok(())
    }

    #[instrument(skip_all,fields(operation=%inv.operation,origin=%inv.origin.public_key,invocation_id=%inv.id,host_id=%inv.host_id))]
    async fn handle_rpc<P>(&self, provider: P, inv: Invocation) -> Result<Vec<u8>, RpcError>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let inv = self.rpc_client.dechunk(inv).await?;
        let (inv, claims) = self.rpc_client.validate_invocation(inv).await?;
        self.validate_provider_invocation(&inv, &claims).await?;
        provider
            .dispatch(
                &Context {
                    actor: Some(inv.origin.public_key.clone()),
                    ..Default::default()
                },
                Message {
                    method: &inv.operation,
                    arg: Cow::from(inv.msg),
                },
            )
            .await
            .map(|m| m.arg.to_vec())
    }

    async fn subscribe_shutdown<P>(
        &self,
        provider: P,
        shutdown_tx: tokio::sync::broadcast::Sender<bool>,
    ) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + 'static,
    {
        let shutdown_topic = format!(
            "wasmbus.rpc.{}.{}.{}.shutdown",
            &self.lattice_prefix, &self.host_data.provider_key, self.host_data.link_name
        );
        debug!("subscribing for shutdown : {}", &shutdown_topic);
        let mut sub = self
            .rpc_client()
            .client()
            .subscribe(shutdown_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        let msg = sub.next().await;
        // TODO: there should be validation on this message, but it's not signed by host yet
        // Shutdown messages are unsigned (see https://github.com/wasmCloud/wasmcloud-otp/issues/256)
        // so we can't verify that this came from a trusted source.
        // When the above issue is fixed, verify the source and keep looping if it's invalid.

        info!("Received termination signal. Shutting down capability provider.");
        // Tell provider to shutdown - before we shut down nats subscriptions,
        // in case it needs to do any message passing during shutdown
        if let Err(e) = provider.shutdown().await {
            error!(error = %e, "got error during provider shutdown processing");
        }
        // send ack to host
        if let Some(crate::anats::Message { reply: Some(reply_to), .. }) = msg.as_ref() {
            let data = b"shutting down".to_vec();
            if let Err(e) = self.rpc_client().publish(reply_to, data).await {
                error!(error = %e, "failed to send shutdown ack");
            }
        }

        // unsubscribe from shutdown topic
        let _ = sub.unsubscribe().await;

        // send shutdown signal to all listeners: quit all subscribers and signal main thread to quit
        if let Err(e) = shutdown_tx.send(true) {
            error!(error = %e, "Problem shutting down:  failure to send signal");
        }

        Ok(())
    }

    async fn subscribe_link_put<P>(&self, provider: P, mut quit: QuitSignal) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let ldput_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.put",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );

        debug!("subscribing for link put : {}", &ldput_topic);
        let mut sub = self
            .rpc_client()
            .client()
            .subscribe(ldput_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        let (this, provider) = (self.clone(), provider.clone());
        process_until_quit!(sub, quit, msg, {
            let span = tracing::error_span!(
                "subscribe_link_put",
                actor_id = tracing::field::Empty,
                provider_id = tracing::field::Empty
            );
            let _enter = span.enter();
            if let Some(ld) = this.parse_msg::<LinkDefinition>(&msg, "link.put") {
                span.record("actor_id", &tracing::field::display(&ld.actor_id));
                span.record("provider_id", &tracing::field::display(&ld.provider_id));
                if this.is_linked(&ld.actor_id).in_current_span().await {
                    warn!("Ignoring duplicate link put");
                } else {
                    info!("Linking actor with provider");
                    match provider.put_link(&ld).in_current_span().await {
                        Ok(true) => {
                            this.put_link(ld).in_current_span().await;
                        }
                        Ok(false) => {
                            // authorization failed or parameters were invalid
                            warn!("put_link denied");
                        }
                        Err(e) => {
                            error!(error = %e, "put_link failed");
                        }
                    }
                }
            } // msg is "link.put"
        }); // process until quit
        Ok(())
    }

    async fn subscribe_link_del<P>(&self, provider: P, mut quit: QuitSignal) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        // Link Delete
        let link_del_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.del",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );
        debug!(topic = %link_del_topic, "subscribing for link del");
        let mut sub = self
            .rpc_client()
            .client()
            .subscribe(link_del_topic.clone())
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        let (this, provider) = (self.clone(), provider.clone());
        process_until_quit!(sub, quit, msg, {
            let span = tracing::trace_span!("subscribe_link_del", topic = %link_del_topic);
            let _enter = span.enter();
            if let Some(ld) = &this.parse_msg::<LinkDefinition>(&msg, "link.del") {
                this.delete_link(&ld.actor_id).in_current_span().await;
                // notify provider that link is deleted
                provider.delete_link(&ld.actor_id).in_current_span().await;
            }
        });
        Ok(())
    }

    async fn subscribe_health<P>(&self, provider: P, mut quit: QuitSignal) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + 'static,
    {
        let topic = format!(
            "wasmbus.rpc.{}.{}.{}.health",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );

        let mut sub = self
            .rpc_client()
            .client()
            .subscribe(topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        let this = self.clone();
        process_until_quit!(sub, quit, msg, {
            let arg = HealthCheckRequest {};
            let resp = match provider.health_request(&arg).await {
                Ok(resp) => resp,
                Err(e) => {
                    error!(error = %e, "error generating health check response");
                    HealthCheckResponse {
                        healthy: false,
                        message: Some(e.to_string()),
                    }
                }
            };
            let buf = if this.host_data.is_test() {
                Ok(serde_json::to_vec(&resp).unwrap())
            } else {
                serialize(&resp)
            };
            match buf {
                Ok(t) => {
                    if let Some(reply_to) = msg.reply.as_ref() {
                        if let Err(e) = this.rpc_client().publish(reply_to, t).await {
                            error!(error = %e, "failed sending health check response");
                        }
                    }
                }
                Err(e) => {
                    // extremely unlikely that InvocationResponse would fail to serialize
                    error!(error = %e, "failed serializing HealthCheckResponse");
                }
            }
        });
        Ok(())
    }

    /// extra validation performed by providers
    async fn validate_provider_invocation(
        &self,
        inv: &Invocation,
        claims: &wascap::prelude::Claims<wascap::jwt::Invocation>,
    ) -> Result<(), String> {
        if !self.host_data.cluster_issuers.contains(&claims.issuer) {
            return Err("Issuer of this invocation is not in list of cluster issuers".into());
        }

        // verify target public key is my key
        if inv.target.public_key != self.host_data.provider_key {
            return Err(format!(
                "target key mismatch: {} != {}",
                &inv.target.public_key, &self.host_data.host_id
            ));
        }

        // verify that the sending actor is linked with this provider
        if !self.is_linked(&inv.origin.public_key).await {
            return Err(format!("unlinked actor: '{}'", &inv.origin.public_key));
        }

        Ok(())
    }
}

pub struct ProviderTransport<'send> {
    pub bridge: &'send HostBridge,
    pub ld: &'send LinkDefinition,
    timeout: StdMutex<Duration>,
}

impl<'send> ProviderTransport<'send> {
    /// constructs a ProviderTransport with the LinkDefinition and bridge.
    /// If the bridge parameter is None, the current (static) bridge is used.
    pub fn new(ld: &'send LinkDefinition, bridge: Option<&'send HostBridge>) -> Self {
        Self::new_with_timeout(ld, bridge, None)
    }

    /// constructs a ProviderTransport with the LinkDefinition, bridge,
    /// and an optional rpc timeout.
    /// If the bridge parameter is None, the current (static) bridge is used.
    pub fn new_with_timeout(
        ld: &'send LinkDefinition,
        bridge: Option<&'send HostBridge>,
        timeout: Option<Duration>,
    ) -> Self {
        #[allow(clippy::redundant_closure)]
        let bridge = bridge.unwrap_or_else(|| crate::provider_main::get_host_bridge());
        let timeout = StdMutex::new(timeout.unwrap_or_else(|| {
            bridge
                .host_data
                .default_rpc_timeout_ms
                .map(|t| Duration::from_millis(t as u64))
                .unwrap_or(DEFAULT_RPC_TIMEOUT_MILLIS)
        }));
        Self { bridge, ld, timeout }
    }
}

#[async_trait]
impl<'send> Transport for ProviderTransport<'send> {
    async fn send(
        &self,
        _ctx: &Context,
        req: Message<'_>,
        _opts: Option<SendOpts>,
    ) -> RpcResult<Vec<u8>> {
        let origin = self.ld.provider_entity();
        let target = self.ld.actor_entity();
        let timeout = {
            if let Ok(rd) = self.timeout.lock() {
                *rd
            } else {
                // if lock is poisioned
                warn!("rpc timeout mutex error - using default value");
                self.bridge
                    .host_data
                    .default_rpc_timeout_ms
                    .map(|t| Duration::from_millis(t as u64))
                    .unwrap_or(DEFAULT_RPC_TIMEOUT_MILLIS)
            }
        };
        self.bridge.rpc_client().send_timeout(origin, target, req, timeout).await
    }

    fn set_timeout(&self, interval: Duration) {
        if let Ok(mut write) = self.timeout.lock() {
            *write = interval;
        } else {
            warn!("rpc timeout mutex error - unchanged")
        }
    }
}
