#![cfg(not(target_arch = "wasm32"))]

use crate::async_nats::{AuthError, ConnectOptions};
use std::io::{BufRead, StderrLock, Write};
use std::str::FromStr;

use once_cell::sync::OnceCell;
#[cfg(feature = "otel")]
use opentelemetry::sdk::{
    trace::{self, IdGenerator, Sampler, Tracer},
    Resource,
};
#[cfg(feature = "otel")]
use opentelemetry::trace::TraceError;
#[cfg(feature = "otel")]
use opentelemetry_otlp::{Protocol, WithExportConfig};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::{DefaultFields, JsonFields};
use tracing_subscriber::fmt::{
    format::{Format, Full, Json, Writer},
    time::SystemTime,
    FmtContext, FormatEvent, FormatFields,
};
use tracing_subscriber::{
    filter::LevelFilter,
    layer::{Layered, SubscriberExt},
    registry::LookupSpan,
    EnvFilter, Layer, Registry,
};

use crate::{
    core::HostData,
    error::RpcError,
    provider::{HostBridge, ProviderDispatch},
};

lazy_static::lazy_static! {
    static ref STDERR: std::io::Stderr = std::io::stderr();
}

static HOST_DATA: OnceCell<HostData> = OnceCell::new();

struct LockedWriter<'a> {
    stderr: StderrLock<'a>,
}

impl<'a> LockedWriter<'a> {
    fn new() -> Self {
        LockedWriter { stderr: STDERR.lock() }
    }
}

impl<'a> Write for LockedWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stderr.write(buf)
    }

    /// DIRTY HACK: when flushing, write a carriage return so the output is clean and then flush
    fn flush(&mut self) -> std::io::Result<()> {
        self.stderr.write_all(&[13])?;
        self.stderr.flush()
    }
}

impl<'a> Drop for LockedWriter<'a> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// singleton host bridge for communicating with the host.
static BRIDGE: OnceCell<HostBridge> = OnceCell::new();

// this may be called any time after initialization
pub fn get_host_bridge() -> &'static HostBridge {
    BRIDGE.get().unwrap_or_else(|| {
        // initialized first thing, so this shouldn't happen
        eprintln!("BRIDGE not initialized");
        panic!();
    })
}

// like get_host_bridge but doesn't panic if it's not initialized
// This could be a valid condition if RpcClient is used outside capability providers
pub fn get_host_bridge_safe() -> Option<&'static HostBridge> {
    BRIDGE.get()
}

#[doc(hidden)]
/// Sets the bridge, return Err if it was already set
pub(crate) fn set_host_bridge(hb: HostBridge) -> Result<(), ()> {
    BRIDGE.set(hb).map_err(|_| ())
}

/// Start provider services: tokio runtime, logger, nats, and rpc subscriptions
pub fn provider_main<P>(
    provider_dispatch: P,
    friendly_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    // get lattice configuration from host
    let host_data = load_host_data().map_err(|e| {
        eprintln!("error loading host data: {}", &e.to_string());
        Box::new(e)
    })?;
    provider_start(provider_dispatch, host_data, friendly_name)
}

/// Start provider services: tokio runtime, logger, nats, and rpc subscriptions,
pub fn provider_start<P>(
    provider_dispatch: P,
    host_data: HostData,
    friendly_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

    runtime.block_on(async { provider_run(provider_dispatch, host_data, friendly_name).await })?;
    // in the unlikely case there are any stuck threads,
    // close them so the process has a clean exit
    runtime.shutdown_timeout(core::time::Duration::from_secs(10));
    Ok(())
}

/// Async provider initialization
pub async fn provider_run<P>(
    provider_dispatch: P,
    host_data: HostData,
    friendly_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    configure_tracing(
        friendly_name.unwrap_or_else(|| host_data.provider_key.clone()),
        host_data.structured_logging,
        host_data.log_level.clone(),
    );

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<bool>(1);

    eprintln!(
        "Starting capability provider {} instance {} with nats url {}",
        &host_data.provider_key, &host_data.instance_id, &host_data.lattice_rpc_url,
    );

    let nats_addr = if !host_data.lattice_rpc_url.is_empty() {
        host_data.lattice_rpc_url.as_str()
    } else {
        crate::provider::DEFAULT_NATS_ADDR
    };
    let nats_server = async_nats::ServerAddr::from_str(nats_addr).map_err(|e| {
        RpcError::InvalidParameter(format!("Invalid nats server url '{nats_addr}': {e}"))
    })?;

    let nc = crate::rpc_client::with_connection_event_logging(
        match (
            host_data.lattice_rpc_user_jwt.trim(),
            host_data.lattice_rpc_user_seed.trim(),
        ) {
            ("", "") => ConnectOptions::default(),
            (rpc_jwt, rpc_seed) => {
                let key_pair = std::sync::Arc::new(nkeys::KeyPair::from_seed(rpc_seed).unwrap());
                let jwt = rpc_jwt.to_owned();
                ConnectOptions::with_jwt(jwt, move |nonce| {
                    let key_pair = key_pair.clone();
                    async move { key_pair.sign(&nonce).map_err(AuthError::new) }
                })
            }
        },
    )
    .connect(nats_server)
    .await?;

    // initialize HostBridge
    let bridge = HostBridge::new_client(nc, &host_data)?;
    set_host_bridge(bridge).ok();
    let bridge = get_host_bridge();

    // pre-populate provider and bridge with initial set of link definitions
    // initialization of any link is fatal for provider startup
    let initial_links = host_data.link_definitions.clone();
    for ld in initial_links.into_iter() {
        if let Err(e) = provider_dispatch.put_link(&ld).await {
            eprintln!(
                "Failed to initialize link during provider startup - ({:?}): {:?}",
                &ld, e
            );
        } else {
            bridge.put_link(ld).await;
        }
    }

    // subscribe to nats topics
    let _join = bridge
        .connect(
            provider_dispatch,
            &shutdown_tx,
            &host_data.lattice_rpc_prefix,
        )
        .await;

    // run until we receive a shutdown request from host
    let _ = shutdown_rx.recv().await;

    // close chunkifiers
    let _ = tokio::task::spawn_blocking(crate::chunkify::shutdown).await;

    // flush async_nats client
    bridge.flush().await;

    Ok(())
}

/// Loads configuration data sent from the host over stdin. The returned host data contains all the
/// configuration information needed to connect to the lattice and any additional configuration
/// provided to this provider (like `config_json`).
///
/// NOTE: this function will read the data from stdin exactly once. If this function is called more
/// than once, it will return a copy of the original data fetched
pub fn load_host_data() -> Result<HostData, RpcError> {
    // TODO(thomastaylor312): Next time we release a non-patch release, we should have this return a
    // borrowed copy of host data instead rather than cloning every time
    HOST_DATA.get_or_try_init(_load_host_data).cloned()
}

// Internal function for populating the host data
pub fn _load_host_data() -> Result<HostData, RpcError> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    {
        let mut handle = stdin.lock();
        handle.read_line(&mut buffer).map_err(|e| {
            RpcError::Rpc(format!(
                "failed to read host data configuration from stdin: {e}"
            ))
        })?;
    }
    // remove spaces, tabs, and newlines before and after base64-encoded data
    let buffer = buffer.trim();
    if buffer.is_empty() {
        return Err(RpcError::Rpc(
            "stdin is empty - expecting host data configuration".to_string(),
        ));
    }
    let bytes = base64::decode(buffer.as_bytes()).map_err(|e| {
        RpcError::Rpc(format!(
            "host data configuration passed through stdin has invalid encoding (expected base64): \
             {e}"
        ))
    })?;
    let host_data: HostData = serde_json::from_slice(&bytes).map_err(|e| {
        RpcError::Rpc(format!(
            "parsing host data: {}:\n{}",
            e,
            String::from_utf8_lossy(&bytes)
        ))
    })?;
    Ok(host_data)
}

#[cfg(feature = "otel")]
const TRACING_PATH: &str = "/v1/traces";

/// A struct that allows us to dynamically choose JSON formatting without using dynamic dispatch.
/// This is just so we avoid any sort of possible slow down in logging code
enum JsonOrNot {
    Not(Format<Full, SystemTime>),
    Json(Format<Json, SystemTime>),
}

impl<S, N> FormatEvent<S, N> for JsonOrNot
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        match self {
            JsonOrNot::Not(f) => f.format_event(ctx, writer, event),
            JsonOrNot::Json(f) => f.format_event(ctx, writer, event),
        }
    }
}

#[cfg(not(feature = "otel"))]
fn configure_tracing(
    _: String,
    structured_logging_enabled: bool,
    log_level_override: Option<String>,
) {
    let base_reg = tracing_subscriber::Registry::default();
    let level_filter = get_level_filter(log_level_override);

    let res = if structured_logging_enabled {
        let log_layer = get_json_log_layer();
        let layered = base_reg.with(level_filter).with(log_layer);
        tracing::subscriber::set_global_default(layered)
    } else {
        let log_layer = get_default_log_layer();
        let layered = base_reg.with(level_filter).with(log_layer);
        tracing::subscriber::set_global_default(layered)
    };

    if let Err(err) = res {
        eprintln!(
            "Logger/tracer was already created by provider, continuing: {}",
            err
        );
    }
}

#[cfg(feature = "otel")]
fn configure_tracing(
    provider_name: String,
    structured_logging_enabled: bool,
    log_level_override: Option<String>,
) {
    let base_reg = tracing_subscriber::Registry::default();
    let level_filter = get_level_filter(log_level_override);

    let maybe_tracer = (std::env::var_os("OTEL_TRACES_EXPORTER")
        .unwrap_or_default()
        .to_ascii_lowercase()
        == "otlp")
        .then(|| get_tracer(provider_name));

    let res = match (maybe_tracer, structured_logging_enabled) {
        (Some(Ok(t)), true) => {
            let log_layer = get_json_log_layer();
            let tracing_layer = tracing_opentelemetry::layer().with_tracer(t);
            let layered = base_reg.with(level_filter).with(log_layer).with(tracing_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Ok(t)), false) => {
            let log_layer = get_default_log_layer();
            let tracing_layer = tracing_opentelemetry::layer().with_tracer(t);
            let layered = base_reg.with(level_filter).with(log_layer).with(tracing_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Err(err)), true) => {
            eprintln!("Unable to configure OTEL tracing, defaulting to logging only: {err:?}");
            let log_layer = get_json_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Err(err)), false) => {
            eprintln!("Unable to configure OTEL tracing, defaulting to logging only: {err:?}");
            let log_layer = get_default_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (None, true) => {
            let log_layer = get_json_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (None, false) => {
            let log_layer = get_default_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
    };

    if let Err(err) = res {
        eprintln!(
            "Logger/tracer was already created by provider, continuing: {}",
            err
        );
    }
}

#[cfg(feature = "otel")]
fn get_tracer(provider_name: String) -> Result<Tracer, TraceError> {
    let mut tracing_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| format!("http://localhost:55681{}", TRACING_PATH));
    if !tracing_endpoint.ends_with(TRACING_PATH) {
        tracing_endpoint.push_str(TRACING_PATH);
    }
    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .http()
                .with_endpoint(tracing_endpoint)
                .with_protocol(Protocol::HttpBinary),
        )
        .with_trace_config(
            trace::config()
                .with_sampler(Sampler::AlwaysOn)
                .with_id_generator(IdGenerator::default())
                .with_max_events_per_span(64)
                .with_max_attributes_per_span(16)
                .with_max_events_per_span(16)
                .with_resource(Resource::new(vec![opentelemetry::KeyValue::new(
                    "service.name",
                    provider_name,
                )])),
        )
        .install_batch(opentelemetry::runtime::Tokio)
}

fn get_default_log_layer() -> impl Layer<Layered<EnvFilter, Registry>> {
    tracing_subscriber::fmt::layer()
        .with_writer(LockedWriter::new)
        .with_ansi(atty::is(atty::Stream::Stderr))
        .event_format(JsonOrNot::Not(Format::default()))
        .fmt_fields(DefaultFields::new())
}

fn get_json_log_layer() -> impl Layer<Layered<EnvFilter, Registry>> {
    tracing_subscriber::fmt::layer()
        .with_writer(LockedWriter::new)
        .with_ansi(atty::is(atty::Stream::Stderr))
        .event_format(JsonOrNot::Json(Format::default().json()))
        .fmt_fields(JsonFields::new())
}

fn get_level_filter(log_level_override: Option<String>) -> EnvFilter {
    let builder = EnvFilter::builder().with_default_directive(LevelFilter::INFO.into());

    let rust_log = std::env::var("RUST_LOG");
    let directives = if let Some(log_level) = log_level_override {
        if rust_log.is_ok() {
            eprintln!("Log level is now provided by the host. RUST_LOG will be ignored");
        }
        log_level
    } else if let Ok(log_level) = rust_log {
        // fallback to env var if host didn't provide level
        log_level
    } else {
        // default
        eprintln!("Log level was not set by host or environment variable.\nDefaulting logger to `info` level");
        "".to_string()
    };

    match builder.parse(directives) {
        Ok(filter) => filter,
        Err(err) => {
            eprintln!("Failed to parse log level: {err:?}\nDefaulting logger to `info` level");
            // NOTE: we considered using parse_lossy here and decided against it, since parse_lossy
            // will use partial successes from the parse. Instead, we want to give the user a clear
            // sign that their log level config couldn't parse
            EnvFilter::default().add_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
        }
    }
}
