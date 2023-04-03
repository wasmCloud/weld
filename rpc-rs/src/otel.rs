//! Contains helpers and code for enabling [OpenTelemetry](https://opentelemetry.io/) tracing for
//! wasmbus-rpc calls. Please note that right now this is only supported for providers. This module
//! is only available with the `otel` feature enabled

use crate::async_nats::{header::HeaderMap, Message as NatsMessage};

use async_nats::header::IntoHeaderName;
use opentelemetry::{
    propagation::{Extractor, Injector, TextMapPropagator},
    sdk::propagation::TraceContextPropagator,
};
use std::collections::HashMap;
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use std::str::FromStr;

lazy_static::lazy_static! {
    static ref EMPTY_HEADERS: HeaderMap = HeaderMap::default();
}

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Extractor`] trait
#[derive(Debug)]
pub struct OtelHeaderExtractor<'a> {
    inner: &'a HashMap<&'a str, &'a str>,
}

impl<'a> OtelHeaderExtractor<'a> {
    /// Creates a new extractor using the given [`HeaderMap`]
    pub fn new(inner: &'a HashMap<&str, &str>) -> Self {
        OtelHeaderExtractor { inner }
    }
}

impl<'a> Extractor for OtelHeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).copied()
    }

    fn keys(&self) -> Vec<&str> {
        self.inner
            .iter()
            // The underlying type is a string and this should never fail, but we unwrap to an empty string anyway
            .map(|(k, _)| std::str::from_utf8(k.as_ref()).unwrap_or_default())
            .collect()
    }
}

impl<'a> AsRef<HashMap<&'a str, &'a str>> for OtelHeaderExtractor<'a> {
    fn as_ref(&self) -> &'a HashMap<&'a str, &'a str> {
        self.inner
    }
}

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Injector`] trait
#[derive(Debug, Default)]
pub struct OtelHeaderInjector<'a> {
    inner: HashMap<&'a str, &'a str>,
}

// impl Into<OtelHeaderInjector> for HeaderMap {
//     fn into(obj: HeaderMap) -> HashMap<String, String> {
//         HashMap::new()
//             // if let Some(h) = obj.headers {
//             //     HashMap::from(
//             //         h.iter()
//             //             .map(|(k, _)| std::str::from_utf8(k.as_ref()).unwrap_or_default())
//             //             .collect()
//             //     )
//             // }
//     }
// }

impl From<OtelHeaderInjector<'_>> for HeaderMap {
    fn from(OtelHeaderInjector { inner }: OtelHeaderInjector) -> HeaderMap {
        let mut headers = async_nats::HeaderMap::new();
        for (k,v) in inner.iter() {
            if let Ok(key) = async_nats::HeaderName::from_str(*k) {
                headers.insert(key,async_nats::HeaderValue::from(*v));
            }
        }
        headers
    }
}

impl<'a> OtelHeaderInjector<'a> {
    /// Creates a new injector using the given [`HeaderMap`]
    pub fn new(headers: HeaderMap) -> Self {
            OtelHeaderInjector {
                inner: HashMap::new()
            }
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into the given header map
    pub fn new_with_span(headers: HeaderMap) -> Self {
        let mut header_map = Self::new(headers);
        header_map.inject_context();
        header_map
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into a default [`HeaderMap`]
    pub fn default_with_span() -> Self {
        let mut header_map = Self::default();
        header_map.inject_context();
        header_map
    }

    /// Injects the current context from the span into the headers
    pub fn inject_context(&mut self) {
        let ctx_propagator = TraceContextPropagator::new();
        ctx_propagator.inject_context(&Span::current().context(), self);
    }
}

impl Injector for OtelHeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.inner.insert(key, value.as_ref());
    }
}

impl<'a, 'b> AsRef<HashMap<&'b str, &'b str>> for OtelHeaderInjector<'b> {
    fn as_ref(&self) -> &HashMap<&'b str, &'b str> {
        &self.inner
    }
}

impl From<HeaderMap> for OtelHeaderInjector<'_> {
    fn from(headers: HeaderMap) -> Self {
        OtelHeaderInjector::new(headers)
    }
}

impl From<OtelHeaderInjector<'_>> for HashMap<&str, &str> {
    fn from(inj: OtelHeaderInjector) -> Self {
        inj.inner
    }
}

/// Structs that have a header map that can be used for span context
pub trait HasHeaderMap {
    /// Extract the header map
    fn header_map(&self) -> HashMap<&str, &str>;
}

impl HasHeaderMap for &NatsMessage {
    fn header_map(&self) -> HashMap<&str, &str> {
        let mut map = HashMap::new();
        if let Some(h) = &self.headers {
            if let Some(parent) = h.get("traceparent".into_header_name()) {
                map.insert("traceparent", parent.as_str());
            }
            if let Some(parent) = h.get("tracestate".into_header_name()) {
                map.insert("tracestate", parent.as_str());
            }
        };
        map
    }
}

/// A convenience function that will extract the current context from NATS message headers and set
/// the parent span for the current tracing Span. If you want to do something more advanced, use the
/// [`OtelHeaderExtractor`] type directly
pub fn attach_span_context(msg: impl HasHeaderMap) {
    let header_map = msg.header_map();
    let headers = OtelHeaderExtractor::new(&header_map);
    let ctx_propagator = TraceContextPropagator::new();
    let parent_ctx = ctx_propagator.extract(&headers);
    Span::current().set_parent(parent_ctx);
}
