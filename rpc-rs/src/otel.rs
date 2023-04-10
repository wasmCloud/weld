//! Contains helpers and code for enabling [OpenTelemetry](https://opentelemetry.io/) tracing for
//! wasmbus-rpc calls. Please note that right now this is only supported for providers. This module
//! is only available with the `otel` feature enabled

use crate::async_nats::header::HeaderMap;
use opentelemetry::{
    propagation::{Extractor, Injector, TextMapPropagator},
    sdk::propagation::TraceContextPropagator,
};
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

lazy_static::lazy_static! {
    static ref EMPTY_HEADERS: HeaderMap = HeaderMap::default();
}

pub const HEADER_TRACEPARENT: &str = "traceparent";
pub const HEADER_TRACESTATE: &str = "tracestate";

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Extractor`] trait
#[derive(Debug)]
pub struct OtelHeaderExtractor<'a> {
    inner: &'a HeaderMap,
}

/// A type to hold the raw data appended to a span
/// to avoid leaking reliance on a specific version of async_nats
struct RawDataExtractor<'a> {
    traceparent: &'a Option<&'a str>,
    tracestate: &'a Option<&'a str>,
}

impl<'a> Extractor for RawDataExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        match key {
            HEADER_TRACEPARENT => *self.traceparent,
            HEADER_TRACESTATE => *self.tracestate,
            _ => None
        }
    }

    fn keys(&self) -> Vec<&str> {
        vec![HEADER_TRACEPARENT, HEADER_TRACESTATE]
    }
}

impl<'a> OtelHeaderExtractor<'a> {
    /// Creates a new extractor using the given [`HeaderMap`]
    pub fn new(headers: &'a HeaderMap) -> Self {
        OtelHeaderExtractor { inner: headers }
    }

    /// Creates a new extractor using the given message
    pub fn new_from_message(msg: &'a crate::async_nats::Message) -> Self {
        let inner = msg.headers.as_ref().unwrap_or(&EMPTY_HEADERS);
        OtelHeaderExtractor { inner }
    }
}

impl<'a> Extractor for OtelHeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).and_then(|s| s.iter().next().map(|s| s.as_str()))
    }

    fn keys(&self) -> Vec<&str> {
        self.inner
            .iter()
            // The underlying type is a string and this should never fail, but we unwrap to an empty string anyway
            .map(|(k, _)| std::str::from_utf8(k.as_ref()).unwrap_or_default())
            .collect()
    }
}

impl<'a> AsRef<HeaderMap> for OtelHeaderExtractor<'a> {
    fn as_ref(&self) -> &'a HeaderMap {
        self.inner
    }
}

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Injector`] trait
#[derive(Debug, Default)]
pub struct OtelHeaderInjector {
    inner: HeaderMap,
}

impl OtelHeaderInjector {
    /// Creates a new injector using the given [`HeaderMap`]
    pub fn new(headers: HeaderMap) -> Self {
        OtelHeaderInjector { inner: headers }
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

impl Injector for OtelHeaderInjector {
    fn set(&mut self, key: &str, value: String) {
        self.inner.insert(key, value.as_ref());
    }
}

impl AsRef<HeaderMap> for OtelHeaderInjector {
    fn as_ref(&self) -> &HeaderMap {
        &self.inner
    }
}

impl From<HeaderMap> for OtelHeaderInjector {
    fn from(headers: HeaderMap) -> Self {
        OtelHeaderInjector::new(headers)
    }
}

impl From<OtelHeaderInjector> for HeaderMap {
    fn from(inj: OtelHeaderInjector) -> Self {
        inj.inner
    }
}

/// A convenience function that will accept raw values tracking parent and state
/// for the current tracing Span. If you want to do something more advanced, use the
/// [`OtelHeaderExtractor`] type directly
pub fn attach_span_context(traceparent: &Option<&str>, tracestate: &Option<&str>) {
    let ctx_propagator = TraceContextPropagator::new();
    let extractor = RawDataExtractor { traceparent, tracestate };
    let parent_ctx = ctx_propagator.extract(&extractor);
    Span::current().set_parent(parent_ctx);
}
