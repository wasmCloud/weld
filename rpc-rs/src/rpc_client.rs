#![cfg(not(target_arch = "wasm32"))]

use std::{
    convert::{TryFrom, TryInto},
    sync::Arc,
    time::Duration,
};

use crate::wascap::{jwt, prelude::Claims};
#[cfg(feature = "prometheus")]
use prometheus::{IntCounter, Opts};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value as JsonValue;
use tracing::{debug, error, instrument, trace, warn};

use crate::{
    chunkify,
    common::Message,
    core::{Invocation, InvocationResponse, WasmCloudEntity},
    error::{RpcError, RpcResult},
};

pub(crate) const DEFAULT_RPC_TIMEOUT_MILLIS: Duration = Duration::from_millis(2000);
/// Amount of time to add to rpc timeout if chunkifying
pub(crate) const CHUNK_RPC_EXTRA_TIME: Duration = Duration::from_secs(13);

/// Send wasmbus rpc messages
///
/// The primary use of RpcClient is providers sending to actors,
/// however providers don't need to construct this - one is created
/// by HostBridge, which providers create during initialization.
///
/// This class is also used by wash and test tools for sending
/// rpc messages to actors and providers. Note that sending to
/// a provider requires an existing link definition, _and_,
/// the message needs to be signed by a valid cluster key.
///
/// This RpcClient does not subscribe to rpc topics.
/// To subscribe, use the nats client api directly.
///
#[derive(Clone)]
pub struct RpcClient {
    client: async_nats::Client,
    key: Arc<wascap::prelude::KeyPair>,
    /// host id (public key) for invocations
    host_id: String,
    /// timeout for rpc messages
    timeout: Option<Duration>,

    #[cfg(feature = "prometheus")]
    pub(crate) stats: Arc<RpcStats>,
}

#[cfg(feature = "prometheus")]
#[derive(Debug)]
pub struct RpcStats {
    // number of rpc nats messages sent
    pub(crate) rpc_sent: IntCounter,
    // number of errors sending - of all types: including errors while receiving responses, and timeouts
    pub(crate) rpc_sent_err: IntCounter,
    // number of messages sent that required chunking
    pub(crate) rpc_sent_chunky: IntCounter,
    // number of responses received that were chunked
    pub(crate) rpc_sent_resp_chunky: IntCounter,
    // total bytes sent (chunked & not chunked). bytes are for sent requests.
    pub(crate) rpc_sent_bytes: IntCounter,
    // total bytes received in response to sent messages
    pub(crate) rpc_sent_resp_bytes: IntCounter,
    // number of timeout errors sending. Note that timeout errors are also included in rpc_sent_err
    pub(crate) rpc_sent_timeouts: IntCounter,

    // number of rpc messages received from rpc subscription
    pub(crate) rpc_recv: IntCounter,
    // recv errors include errors receiving subscription messages and replying to them
    pub(crate) rpc_recv_err: IntCounter,
    // number of rpc messages received that were chunkified
    pub(crate) rpc_recv_chunky: IntCounter,
    // number of rpc message responses that were chunkified
    pub(crate) rpc_recv_resp_chunky: IntCounter,
    // bytes received in rpc (subscription) messages
    pub(crate) rpc_recv_bytes: IntCounter,
    // bytes sent in response to received rpc messages
    pub(crate) rpc_recv_resp_bytes: IntCounter,
}
/// Returns the rpc topic (subject) name for sending to an actor or provider.
/// A provider entity must have the public_key and link_name fields filled in.
/// An actor entity must have a public_key and an empty link_name.
#[doc(hidden)]
pub fn rpc_topic(entity: &WasmCloudEntity, lattice_prefix: &str) -> String {
    if !entity.link_name.is_empty() {
        // provider target
        format!(
            "wasmbus.rpc.{}.{}.{}",
            lattice_prefix, entity.public_key, entity.link_name
        )
    } else {
        // actor target
        format!("wasmbus.rpc.{}.{}", lattice_prefix, entity.public_key)
    }
}

impl RpcClient {
    /// Constructs a new RpcClient with an async nats connection.
    /// parameters: async nats client, rpc timeout
    /// secret key for signing messages, host_id, and optional timeout.
    pub fn new(
        nats: async_nats::Client,
        host_id: String,
        timeout: Option<Duration>,
        key_pair: Arc<wascap::prelude::KeyPair>,
    ) -> Self {
        Self::new_client(nats, host_id, timeout, key_pair)
    }

    /// Constructs a new RpcClient with a nats connection.
    /// parameters: nats client, lattice rpc prefix (usually "default"),
    /// secret key for signing messages, host_id, and optional timeout.
    pub(crate) fn new_client(
        nats: async_nats::Client,
        host_id: String,
        timeout: Option<Duration>,
        key_pair: Arc<wascap::prelude::KeyPair>,
    ) -> Self {
        RpcClient {
            client: nats,
            host_id,
            timeout,
            #[cfg(feature = "prometheus")]
            stats: Arc::new(RpcStats::init(key_pair.public_key())),
            key: key_pair,
        }
    }

    /// convenience method for returning async client
    pub fn client(&self) -> async_nats::Client {
        self.client.clone()
    }

    /// Replace the default timeout with the specified value.
    /// If the parameter is None, unsets the default timeout
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Send an rpc message using json-encoded data
    pub async fn send_json<Target, Arg, Resp>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        lattice: &str,
        method: &str,
        data: JsonValue,
    ) -> RpcResult<JsonValue>
    where
        Arg: DeserializeOwned + Serialize,
        Resp: DeserializeOwned + Serialize,
        Target: Into<WasmCloudEntity>,
    {
        let msg = JsonMessage(method, data).try_into()?;
        let bytes = self.send(origin, target, lattice, msg).await?;
        let resp = response_to_json::<Resp>(&bytes)?;
        Ok(resp)
    }

    /// Send a wasmbus rpc message by wrapping with an Invocation before sending over nats.
    /// 'target' may be &str or String for sending to an actor, or a WasmCloudEntity (for actor or provider)
    /// If a response is not received within the default timeout, the Error RpcError::Timeout is returned.
    /// If the client timeout has been set, this call is equivalent to send_timeout passing in the
    /// default timeout.
    pub async fn send<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        lattice: &str,
        message: Message<'_>,
    ) -> RpcResult<Vec<u8>>
    where
        Target: Into<WasmCloudEntity>,
    {
        let rc = self.inner_rpc(origin, target, lattice, message, true, self.timeout).await;
        #[cfg(feature = "prometheus")]
        {
            if rc.is_err() {
                self.stats.rpc_sent_err.inc()
            }
        }
        rc
    }

    /// Send a wasmbus rpc message, with a timeout.
    /// The rpc message is wrapped with an Invocation before sending over nats.
    /// 'target' may be &str or String for sending to an actor, or a WasmCloudEntity (for actor or provider)
    /// If the timeout expires before the response is received, this returns Error RpcError::Timeout.
    pub async fn send_timeout<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        lattice: &str,
        message: Message<'_>,
        timeout: Duration,
    ) -> RpcResult<Vec<u8>>
    where
        Target: Into<WasmCloudEntity>,
    {
        let rc = self.inner_rpc(origin, target, lattice, message, true, Some(timeout)).await;
        #[cfg(feature = "prometheus")]
        {
            if rc.is_err() {
                self.stats.rpc_sent_err.inc();
            }
        }
        rc
    }

    /// Send a wasmbus rpc message without waiting for response.
    /// This has somewhat limited utility and is only useful if
    /// the message is declared to return no args, or if the caller
    /// doesn't care about the response.
    /// 'target' may be &str or String for sending to an actor,
    /// or a WasmCloudEntity (for actor or provider)
    #[doc(hidden)]
    pub async fn post<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        lattice: &str,
        message: Message<'_>,
    ) -> RpcResult<()>
    where
        Target: Into<WasmCloudEntity>,
    {
        let rc = self.inner_rpc(origin, target, lattice, message, false, None).await;
        match rc {
            Err(e) => {
                #[cfg(feature = "prometheus")]
                self.stats.rpc_sent_err.inc();
                Err(e)
            }
            Ok(_) => Ok(()),
        }
    }

    /// request or publish an rpc invocation
    #[instrument(level = "debug", skip(self, origin, target, message), fields(issuer = tracing::field::Empty, origin_url = tracing::field::Empty, subject = tracing::field::Empty, target_url = tracing::field::Empty, method = tracing::field::Empty))]
    async fn inner_rpc<Target>(
        &self,
        origin: WasmCloudEntity,
        target: Target,
        lattice: &str,
        message: Message<'_>,
        expect_response: bool,
        timeout: Option<Duration>,
    ) -> RpcResult<Vec<u8>>
    where
        Target: Into<WasmCloudEntity>,
    {
        let target = target.into();
        let origin_url = origin.url();
        let subject = make_uuid();
        let issuer = self.key.public_key();
        let raw_target_url = target.url();
        let target_url = format!("{}/{}", raw_target_url, &message.method);

        // Record all of the fields on the span. To avoid extra allocations, we are only going to
        // record here after we generate/derive the values
        let current_span = tracing::span::Span::current();
        current_span.record("issuer", &tracing::field::display(issuer.clone()));
        current_span.record("origin_url", &tracing::field::display(&origin_url));
        current_span.record("subject", &tracing::field::display(&subject));
        current_span.record("target_url", &tracing::field::display(&raw_target_url));
        current_span.record("method", &tracing::field::display(message.method));

        debug!("rpc_client sending");
        let claims = Claims::<jwt::Invocation>::new(
            issuer.clone(),
            subject.clone(),
            &target_url,
            &origin_url,
            &invocation_hash(&target_url, &origin_url, message.method, &message.arg),
        );

        let topic = rpc_topic(&target, lattice);
        let method = message.method.to_string();
        let len = message.arg.len();
        let chunkify = chunkify::needs_chunking(len);

        #[allow(unused_variables)]
        let (invocation, body) = {
            let mut inv = Invocation {
                origin,
                target,
                operation: method.clone(),
                id: subject,
                encoded_claims: claims.encode(&self.key).unwrap_or_default(),
                host_id: self.host_id.clone(),
                content_length: Some(len as u64),
                ..Default::default()
            };
            if chunkify {
                (inv, Some(Vec::from(message.arg)))
            } else {
                inv.msg = Vec::from(message.arg);
                (inv, None)
            }
        };
        let nats_body = crate::common::serialize(&invocation)?;
        if let Some(body) = body {
            let inv_id = invocation.id.clone();
            debug!(invocation_id = %inv_id, %len, "chunkifying invocation");
            // start chunking thread
            let lattice = lattice.to_string();
            tokio::task::spawn_blocking(move || {
                let ce = chunkify::chunkify_endpoint(None, lattice)?;
                ce.chunkify(&inv_id, &mut body.as_slice())
            })
            // I tried starting the send to ObjectStore in background thread,
            // and then send the rpc, but if the objectstore hasn't completed,
            // the recipient gets a missing object error,
            // so we need to flush this first
            .await
            // any errors sending chunks will cause send to fail with RpcError::Nats
            .map_err(|e| RpcError::Other(e.to_string()))??;
        }

        let timeout = if chunkify {
            timeout.map(|t| t + CHUNK_RPC_EXTRA_TIME)
        } else {
            timeout
        };

        #[cfg(feature = "prometheus")]
        {
            self.stats.rpc_sent.inc();
            if let Some(len) = invocation.content_length {
                self.stats.rpc_sent_bytes.inc_by(len);
            }
            if chunkify {
                self.stats.rpc_sent_chunky.inc();
            }
        }
        if expect_response {
            let payload = if let Some(timeout) = timeout {
                // if we expect a response before timeout, finish sending all chunks, then wait for response
                match tokio::time::timeout(timeout, self.request(topic, nats_body)).await {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(rpc_err)) => Err(RpcError::Nats(format!(
                        "rpc send error: {}: {}",
                        target_url, rpc_err
                    ))),
                    Err(timeout_err) => {
                        #[cfg(feature = "prometheus")]
                        {
                            self.stats.rpc_sent_timeouts.inc();
                        }
                        error!(error = %timeout_err, "rpc timeout: sending to target");
                        Err(RpcError::Timeout(format!(
                            "sending to {}: {}",
                            &target_url, timeout_err
                        )))
                    }
                }
            } else {
                // no timeout, wait indefinitely or until host times out
                self.request(topic, nats_body)
                    .await
                    .map_err(|e| RpcError::Nats(format!("rpc send error: {}: {}", target_url, e)))
            }?;

            let inv_response =
                crate::common::deserialize::<InvocationResponse>(&payload).map_err(|e| {
                    RpcError::Deser(format!("response to {}: {}", &method, &e.to_string()))
                })?;
            match inv_response.error {
                None => {
                    #[cfg(feature = "prometheus")]
                    {
                        if let Some(len) = inv_response.content_length {
                            self.stats.rpc_sent_resp_bytes.inc_by(len);
                        }
                    }
                    // was response chunked?
                    let msg = if inv_response.content_length.is_some()
                        && inv_response.content_length.unwrap() > inv_response.msg.len() as u64
                    {
                        let lattice = lattice.to_string();
                        #[cfg(feature = "prometheus")]
                        {
                            self.stats.rpc_sent_resp_chunky.inc();
                        }
                        tokio::task::spawn_blocking(move || {
                            let ce = chunkify::chunkify_endpoint(None, lattice)?;
                            ce.get_unchunkified_response(&inv_response.invocation_id)
                        })
                        .await
                        .map_err(|je| RpcError::Other(format!("join/resp-chunk: {}", je)))??
                    } else {
                        inv_response.msg
                    };
                    trace!("rpc ok response");
                    Ok(msg)
                }
                Some(err) => {
                    // if error is Some(_), we must ignore the msg field
                    error!(error = %err, "rpc error response");
                    Err(RpcError::Rpc(err))
                }
            }
        } else {
            self.publish(&topic, nats_body)
                .await
                .map_err(|e| RpcError::Nats(format!("rpc send error: {}: {}", target_url, e)))?;
            Ok(Vec::new())
        }
    }

    /// Send a nats message and wait for the response.
    /// This can be used for general nats messages, not just wasmbus actor/provider messages.
    /// If this client has a default timeout, and a response is not received within
    /// the appropriate time, an error will be returned.
    pub async fn request(&self, subject: String, data: Vec<u8>) -> RpcResult<Vec<u8>> {
        let nc = self.client();
        let fut = nc.request(subject, data.into());
        let bytes = if let Some(timeout) = &self.timeout {
            tokio::time::timeout(*timeout, fut).await.map_err(|_| {
                warn!(
                    timeout_sec = timeout.as_secs_f32(),
                    "request timeout exceeded"
                );
                RpcError::Timeout(format!(
                    "request timeout exceeded: {} sec",
                    timeout.as_secs_f32()
                ))
            })?
        } else {
            fut.await
        }
        .map_err(|e| RpcError::Nats(e.to_string()))?
        .payload;
        Ok(bytes.to_vec())
    }

    /// Send a nats message with no reply-to. Do not wait for a response.
    /// This can be used for general nats messages, not just wasmbus actor/provider messages.
    pub async fn publish(&self, subject: &str, data: Vec<u8>) -> RpcResult<()> {
        self.client()
            .publish(subject.to_string(), data.into())
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))
    }

    pub async fn publish_invocation_response(
        &self,
        reply_to: String,
        response: InvocationResponse,
        lattice: &str,
    ) -> Result<(), String> {
        let content_length = Some(response.msg.len() as u64);
        let response = {
            let inv_id = response.invocation_id.clone();
            if chunkify::needs_chunking(response.msg.len()) {
                #[cfg(feature = "prometheus")]
                {
                    self.stats.rpc_recv_resp_chunky.inc();
                }
                let msg = response.msg;
                let lattice = lattice.to_string();
                tokio::task::spawn_blocking(move || {
                    let ce = chunkify::chunkify_endpoint(None, lattice)
                        .map_err(|e| format!("connecting for chunkifying: {}", &e.to_string()))?;
                    ce.chunkify_response(&inv_id, &mut msg.as_slice())
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|je| format!("join/response-chunk: {}", je))??;
                InvocationResponse {
                    msg: Vec::new(),
                    content_length,
                    ..response
                }
            } else {
                InvocationResponse { content_length, ..response }
            }
        };

        match crate::common::serialize(&response) {
            Ok(t) => {
                if let Err(e) = self.client().publish(reply_to.clone(), t.into()).await {
                    error!(
                        %reply_to,
                        error = %e,
                        "failed sending rpc response",
                    );
                }
            }
            Err(e) => {
                // extremely unlikely that InvocationResponse would fail to serialize
                error!(error = %e, "failed serializing InvocationResponse");
            }
        }
        Ok(())
    }

    pub async fn dechunk(&self, mut inv: Invocation, lattice: &str) -> RpcResult<Invocation> {
        if inv.content_length.is_some() && inv.content_length.unwrap() > inv.msg.len() as u64 {
            #[cfg(feature = "prometheus")]
            {
                self.stats.rpc_recv_chunky.inc();
            }
            let inv_id = inv.id.clone();
            let lattice = lattice.to_string();
            inv.msg = tokio::task::spawn_blocking(move || {
                let ce = chunkify::chunkify_endpoint(None, lattice)
                    .map_err(|e| format!("connecting for de-chunkifying: {}", &e.to_string()))?;
                ce.get_unchunkified(&inv_id).map_err(|e| e.to_string())
            })
            .await
            .map_err(|je| format!("join/dechunk-validate: {}", je))??;
        }
        Ok(inv)
    }

    /// Initial validation of received message. See provider::validate_provider_invocation for second part.
    pub async fn validate_invocation(
        &self,
        inv: Invocation,
    ) -> Result<(Invocation, Claims<jwt::Invocation>), String> {
        let vr = jwt::validate_token::<jwt::Invocation>(&inv.encoded_claims)
            .map_err(|e| format!("{}", e))?;
        if vr.expired {
            return Err("Invocation claims token expired".into());
        }
        if !vr.signature_valid {
            return Err("Invocation claims signature invalid".into());
        }
        if vr.cannot_use_yet {
            return Err("Attempt to use invocation before claims token allows".into());
        }
        let target_url = format!("{}/{}", inv.target.url(), &inv.operation);
        let hash = invocation_hash(&target_url, &inv.origin.url(), &inv.operation, &inv.msg);
        let claims =
            Claims::<jwt::Invocation>::decode(&inv.encoded_claims).map_err(|e| format!("{}", e))?;
        let inv_claims = claims
            .metadata
            .as_ref()
            .ok_or_else(|| "No wascap metadata found on claims".to_string())?;
        if inv_claims.invocation_hash != hash {
            return Err(format!(
                "Invocation hash does not match signed claims hash ({} / {})",
                inv_claims.invocation_hash, hash
            ));
        }
        if !inv.host_id.starts_with('N') && inv.host_id.len() != 56 {
            return Err(format!("Invalid host ID on invocation: '{}'", inv.host_id));
        }

        if inv_claims.target_url != target_url {
            return Err(format!(
                "Invocation claims and invocation target URL do not match: {} != {}",
                &inv_claims.target_url, &target_url
            ));
        }
        if inv_claims.origin_url != inv.origin.url() {
            return Err("Invocation claims and invocation origin URL do not match".into());
        }
        Ok((inv, claims))
    }
}

#[derive(Clone)]
pub struct InvocationArg {
    /// Sender of the message
    pub origin: String,

    /// Method name, usually of the form Service.Method
    pub operation: String,

    /// Message payload (could be empty array). May need to be serialized
    pub arg: Vec<u8>,
}

pub(crate) fn invocation_hash(
    target_url: &str,
    origin_url: &str,
    method: &str,
    args: &[u8],
) -> String {
    use sha2::Digest as _;

    let mut hasher = sha2::Sha256::new();
    hasher.update(origin_url.as_bytes());
    hasher.update(target_url.as_bytes());
    hasher.update(method.as_bytes());
    hasher.update(args);
    let digest = hasher.finalize();
    data_encoding::HEXUPPER.encode(digest.as_slice())
}

/// Create a new random uuid for invocations.
/// Internally this (currently) uses the uuid crate, which uses 'getrandom',
/// which uses the operating system's random number generator.
/// See https://docs.rs/getrandom/0.2.3/getrandom/ for details
#[doc(hidden)]
pub fn make_uuid() -> String {
    use uuid::Uuid;
    // uuid uses getrandom, which uses the operating system's RNG
    // as the source of random numbers.
    Uuid::new_v4()
        .as_simple()
        .encode_lower(&mut Uuid::encode_buffer())
        .to_string()
}

/// A Json message (method, args)
struct JsonMessage<'m>(&'m str, JsonValue);

impl<'m> TryFrom<JsonMessage<'m>> for Message<'m> {
    type Error = RpcError;

    /// convert json message to rpc message (msgpack)
    fn try_from(jm: JsonMessage<'m>) -> Result<Message<'m>, Self::Error> {
        let arg = json_to_args::<JsonValue>(jm.1)?;
        Ok(Message {
            method: jm.0,
            arg: std::borrow::Cow::Owned(arg),
        })
    }
}

/// convert json args to msgpack
fn json_to_args<T>(v: JsonValue) -> RpcResult<Vec<u8>>
where
    T: Serialize,
    T: DeserializeOwned,
{
    crate::common::serialize(
        &serde_json::from_value::<T>(v)
            .map_err(|e| RpcError::Deser(format!("invalid params: {}.", e)))?,
    )
}

/// convert message response to json
fn response_to_json<T>(msg: &[u8]) -> RpcResult<JsonValue>
where
    T: Serialize,
    T: DeserializeOwned,
{
    serde_json::to_value(crate::common::deserialize::<T>(msg)?)
        .map_err(|e| RpcError::Ser(format!("response serialization : {}.", e)))
}

#[cfg(feature = "prometheus")]
impl RpcStats {
    fn init(public_key: String) -> RpcStats {
        let mut map = std::collections::HashMap::new();
        map.insert("public_key".to_string(), public_key);

        RpcStats {
            rpc_sent: IntCounter::with_opts(
                Opts::new("rpc_sent", "number of rpc nats messages sent").const_labels(map.clone()),
            )
            .unwrap(),
            rpc_sent_err: IntCounter::with_opts(
                Opts::new("rpc_sent_err", "number of errors sending rpc").const_labels(map.clone()),
            )
            .unwrap(),
            rpc_sent_chunky: IntCounter::with_opts(
                Opts::new(
                    "rpc_sent_chunky",
                    "number of rpc messages that were chunkified",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_sent_resp_chunky: IntCounter::with_opts(
                Opts::new(
                    "rpc_sent_resp_chunky",
                    "number of responses to sent rpc that were chunkified",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_sent_bytes: IntCounter::with_opts(
                Opts::new("rpc_sent_bytes", "total bytes sent in rpc requests")
                    .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_sent_resp_bytes: IntCounter::with_opts(
                Opts::new(
                    "rpc_sent_resp_bytes",
                    "total bytes sent in responses to incoming rpc",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_sent_timeouts: IntCounter::with_opts(
                Opts::new(
                    "rpc_sent_timeouts",
                    "number of rpc messages that incurred timeout error",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_recv: IntCounter::with_opts(
                Opts::new("rpc_recv", "number of rpc messages received").const_labels(map.clone()),
            )
            .unwrap(),
            rpc_recv_err: IntCounter::with_opts(
                Opts::new(
                    "rpc_recv_err",
                    "number of errors encountered responding to incoming rpc",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_recv_chunky: IntCounter::with_opts(
                Opts::new(
                    "rpc_recv_chunky",
                    "number of received rpc that were chunkified",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_recv_resp_chunky: IntCounter::with_opts(
                Opts::new(
                    "rpc_recv_resp_chunky",
                    "number of chunkified responses to received rpc",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_recv_bytes: IntCounter::with_opts(
                Opts::new("rpc_recv_bytes", "total bytes in received rpc")
                    .const_labels(map.clone()),
            )
            .unwrap(),
            rpc_recv_resp_bytes: IntCounter::with_opts(
                Opts::new(
                    "rpc_recv_resp_bytes",
                    "total bytes in responses to incoming rpc",
                )
                .const_labels(map.clone()),
            )
            .unwrap(),
        }
    }
}
