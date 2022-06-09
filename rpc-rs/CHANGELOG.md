# wasmbus-rpc Changelog

## BREAKING CHANGES from 0.8.x to 0.9.0

- nats-aflowt is replaced with async-nats!
  - wasmbus_rpc::anats re-exports async_nats, not nats_aflowt
  - anats::ServerAddress renamed to anats::ServerAddr
  - anats::Subscription is not public, replaced with anats::Subscriber
  - anats::Subscription.close() replaced with anats::Subscriber.unsubscribe()
  - anats::Options renamed to anats::ConnectOptions
  - anats::Connection is no longer public. Use anats::Client instead.
  - anats::Message.data renamed to .payload
- HostBridge::new() changes
  - first parameter is anats::Client instead of anats::Connection
- RpcClient::new() changes 
  - new() parameter takes async_nats Client instead of anats::Client
  - lattice prefix removed from constructor, added in to some of the method parameters
- got rid of enum NatsClientType, replaced with async_nats::Client
- removed feature "chunkify" (it is always enabled for non-wasm32 targets)

- RpcError does not implement Serialize, Deserialize, or PartialEq
  (unless/until we can find a good reason to support these) 


## non-breaking changes
- upgraded minicbor to 0.16-rc1
- replaced ring with sha2 for sha256
- upgraded uuid


## 0.7.0

### Breaking changes (since 0.6.x)

- Some of the crate's exported symbols have moved to submodules. The intent is to resolve some linking problems
  resulting from multiple inconsistent references to these symbols.
  Most of these changes will require only a recompile, for Actors and Providers 
  that import `wasmbus_rpc::actor::prelude::*` or `wasmbus_rpc::provider::prelude::*`, respectively.
  - wasmbus_rpc::{RpcError,RpcResult} -> wasmbus_rpc::error::{RpcError,RpcResult}
  - wasmbus_rpc::{Message,MessageDispatch,Transport} -> wasmbus_rpc::common::{Message,MessageDispatch,Transport}
  - wasmbus_rpc::context::Context -> wasmbus_rpc::common::Context
  - To help avoid external breakage, the crate-level symbols have been marked deprecated
  
- removed feature options [ser_json] and [ser_msgpack] - ser_msgpack was always, and remains, the default.
- added a `cbor` module to wrap `minicor`, so the choice of cbor implementation is not exposed.
- Depends on codegen-0.3.0


## 0.7.0-alpha.1

### Features

- replaced `ratsio` with `nats-aflowt`
  - `nats-aflowt` is reexported as `wasmbus_rpc::anats`
- removed dependency on nats-io/nats.rs (official nats client crate)
- `RpcClient::request` obeys the `timeout` parameter in `RpcClient::new(..)`

### Breaking changes (since 0.6.x)

- removed support for rpc_client types `Sync` and `Asynk`. Only `Async` now.
- `provider::NatsClient` changed type and is `anats::Connection`
- type `Subscription` is no longer exported (now: anats::Subscription)
- `HostBridge::new` - nats parameter no longer enclosed in Arc<>
- `get_async` returns `Option<anats::Connection>` instead of `Option<Arc<NatsClient>>`
