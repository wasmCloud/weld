use crate::error::{RpcError, RpcResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::HashMap, fmt};

/// A wasmcloud message
#[derive(Debug)]
pub struct Message<'m> {
    /// Message name, usually in the form 'Trait.method'
    pub method: &'m str,
    /// parameter serialized as a byte array. If the method takes no args, the array will be
    /// zero length.
    pub arg: Cow<'m, [u8]>,
}

/// Context - message passing metadata used by wasmhost Actors and Capability Providers
#[derive(Default, Debug, Clone)]
pub struct Context {
    /// Messages received by Context Provider will have actor set to the actor's public key
    pub actor: Option<String>,

    /// Span name/context for tracing. This is a placeholder for now
    pub span: Option<String>,
}

/// Client config defines the intended recipient of a message and parameters that transport may use to adapt sending it
#[derive(Default, Debug)]
pub struct SendOpts {
    /// Optional flag for idempotent messages - transport may perform retries within configured timeouts
    pub idempotent: bool,

    /// Optional flag for read-only messages - those that do not change the responder's state. read-only messages may be retried within configured timeouts.
    pub read_only: bool,
}

impl SendOpts {
    #[must_use]
    pub fn idempotent(mut self, val: bool) -> SendOpts {
        self.idempotent = val;
        self
    }

    #[must_use]
    pub fn read_only(mut self, val: bool) -> SendOpts {
        self.read_only = val;
        self
    }
}

/// Transport determines how messages are sent
/// Alternate implementations could be mock-server, or test-fuzz-server / test-fuzz-client
#[async_trait]
pub trait Transport: Send {
    async fn send(
        &self,
        ctx: &Context,
        req: Message<'_>,
        opts: Option<SendOpts>,
    ) -> std::result::Result<Vec<u8>, RpcError>;

    /// Sets rpc timeout
    fn set_timeout(&self, interval: std::time::Duration);
}

// select serialization/deserialization mode
pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T, RpcError> {
    rmp_serde::from_read_ref(buf).map_err(|e| RpcError::Deser(e.to_string()))
}

pub fn serialize<T: Serialize>(data: &T) -> Result<Vec<u8>, RpcError> {
    rmp_serde::to_vec_named(data).map_err(|e| RpcError::Ser(e.to_string()))
    // for benchmarking: the following line uses msgpack without field names
    //rmp_serde::to_vec(data).map_err(|e| RpcError::Ser(e.to_string()))
}

#[async_trait]
pub trait MessageDispatch {
    async fn dispatch<'disp, 'ctx, 'msg>(
        &'disp self,
        ctx: &'ctx Context,
        message: Message<'msg>,
    ) -> Result<Message<'msg>, RpcError>;
}

/// Message encoding format
#[derive(Clone, PartialEq)]
pub enum MessageFormat {
    Msgpack,
    Cbor,
    Empty,
    Unknown,
}

impl fmt::Display for MessageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MessageFormat::Msgpack => "msgpack",
            MessageFormat::Cbor => "cbor",
            MessageFormat::Empty => "empty",
            MessageFormat::Unknown => "unknown",
        })
    }
}

impl fmt::Debug for MessageFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string())
    }
}

impl MessageFormat {
    pub fn write_header<W: std::io::Write>(&self, mut buf: W) -> std::io::Result<usize> {
        match self {
            MessageFormat::Cbor => buf.write(&[127u8]),    // 0x7f
            MessageFormat::Msgpack => buf.write(&[193u8]), // 0xc1
            MessageFormat::Empty => Ok(0),
            MessageFormat::Unknown => Ok(0),
        }
    }
}

/// returns serialization format,
/// and offset for beginning of payload
pub fn message_format(data: &[u8]) -> (MessageFormat, usize) {
    // The initial byte of the message is used to distinguish between
    // a legacy msgpack payload, and a prefix plus payload.
    // If the payload has only a single byte, it must be msgpack.
    // A wasmbus msgpack payload containing 2 or more bytes will never begin
    // with any of the following:
    //   0x00-0x7f - used by msgpack to encode small ints 0-127
    //   0xc0      - msgpack Nil
    //   0xc1      - unused by msgpack
    // These values can all be used as the initial byte of a prefix.
    //
    // (The reason the first byte cannot be a msgpack small int is that
    //  wasmbus payloads only contain a single value: a primitive type,
    //  or a struct or a map. It cannot be a Nil because the single value
    //  of a wasmbus message is never an Option<> and there is no other
    //  way that a null would be generated.)
    match data.len() {
        0 => (MessageFormat::Empty, 0),
        1 => (MessageFormat::Msgpack, 0), // 1-byte msgpack legacy
        _ => {
            match data[0] {
                0x7f => (MessageFormat::Cbor, 1),           // prefix + cbor
                0xc1 => (MessageFormat::Msgpack, 1),        // prefix + msgpack
                0x00..=0x7e => (MessageFormat::Unknown, 0), // RESERVED
                0xc0 => (MessageFormat::Unknown, 0),        // RESERVED
                _ => (MessageFormat::Msgpack, 0),           // legacy
            }
        }
    }
}

pub type CborDecodeFn<T> = dyn Fn(&mut crate::cbor::Decoder<'_>) -> RpcResult<T>;

pub fn decode<T: serde::de::DeserializeOwned>(
    buf: &[u8],
    cbor_dec: &CborDecodeFn<T>,
) -> RpcResult<T> {
    let value = match message_format(buf) {
        (MessageFormat::Cbor, offset) => {
            let d = &mut crate::cbor::Decoder::new(&buf[offset..]);
            cbor_dec(d)?
        }
        (MessageFormat::Msgpack, offset) => deserialize(&buf[offset..])
            .map_err(|e| RpcError::Deser(format!("decoding '{}': {{}}", e)))?,
        _ => return Err(RpcError::Deser("invalid encoding for '{}'".to_string())),
    };
    Ok(value)
}

pub trait DecodeOwned: for<'de> crate::minicbor::Decode<'de> {}
impl<T> DecodeOwned for T where T: for<'de> crate::minicbor::Decode<'de> {}

/// Wasmbus rpc sender that can send any message and cbor-serializable payload
/// requires Protocol="2"
pub struct AnySender<T: Transport> {
    transport: T,
}

impl<T: Transport> AnySender<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: Transport + Sync + Send> AnySender<T> {
    /// Send enoded payload
    #[inline]
    async fn send_raw<'s, 'ctx, 'msg>(
        &'s self,
        ctx: &'ctx Context,
        msg: Message<'msg>,
    ) -> RpcResult<Vec<u8>> {
        self.transport.send(ctx, msg, None).await
    }

    /// Send rpc with serializable payload
    pub async fn send<In: Serialize, Out: serde::de::DeserializeOwned>(
        &self,
        ctx: &Context,
        method: &str,
        arg: &In,
    ) -> RpcResult<Out> {
        let mut buf = Vec::new();
        MessageFormat::Cbor.write_header(&mut buf).unwrap();
        minicbor_ser::to_writer(arg, &mut buf).map_err(|e| RpcError::Ser(e.to_string()))?;
        let resp = self
            .send_raw(
                ctx,
                Message {
                    method,
                    arg: Cow::Borrowed(&buf),
                },
            )
            .await?;
        let result: Out =
            minicbor_ser::from_slice(&resp).map_err(|e| RpcError::Deser(e.to_string()))?;
        Ok(result)
    }

    /// Send rpc with serializable payload using cbor encode/decode
    pub async fn send_cbor<'de, In: crate::minicbor::Encode, Out: DecodeOwned>(
        &self,
        ctx: &Context,
        method: &str,
        arg: &In,
    ) -> RpcResult<Out> {
        let mut buf = Vec::new();
        MessageFormat::Cbor.write_header(&mut buf).unwrap();
        crate::minicbor::encode(arg, &mut buf).map_err(|e| RpcError::Ser(e.to_string()))?;
        let resp = self
            .send_raw(
                ctx,
                Message {
                    method,
                    arg: Cow::Borrowed(&buf),
                },
            )
            .await?;
        let result: Out =
            crate::minicbor::decode(&resp).map_err(|e| RpcError::Deser(e.to_string()))?;
        Ok(result)
    }
}

pub type Unit = ();

/// Document Type
///
/// Document types represents protocol-agnostic open content that is accessed like JSON data.
/// Open content is useful for modeling unstructured data that has no schema, data that can't be
/// modeled using rigid types, or data that has a schema that evolves outside of the purview of a model.
/// The serialization format of a document is an implementation detail of a protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum Document<'v> {
    /// JSON object
    Object(std::collections::HashMap<String, Document<'v>>),
    /// Blob
    Blob(Cow<'v, [u8]>),
    /// JSON array
    Array(Vec<Document<'v>>),
    /// JSON number
    Number(Number),
    /// JSON string
    String(Cow<'v, str>),
    /// JSON boolean
    Bool(bool),
    /// JSON null
    Null,
}

impl<'v> Document<'v> {
    /// Returns the map, if Document is an Object, otherwise None
    pub fn to_object(self) -> Option<std::collections::HashMap<String, Document<'v>>> {
        if let Document::Object(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns reference to the map, if Document is an Object, otherwise None
    pub fn as_object(&self) -> Option<&std::collections::HashMap<String, Document<'v>>> {
        if let Document::Object(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is an object
    pub fn is_object(&self) -> bool {
        self.as_object().is_some()
    }

    /// Returns the blob, if Document is an Blob, otherwise None
    pub fn as_blob(&self) -> Option<&Cow<'v, [u8]>> {
        if let Document::Blob(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns the blob, if Document is an Blob, otherwise None
    pub fn to_blob(self) -> Option<Cow<'v, [u8]>> {
        if let Document::Blob(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is a blob (byte array)
    pub fn is_blob(&self) -> bool {
        self.as_blob().is_some()
    }

    /// Returns the array, if Document is an Array, otherwise None
    pub fn as_array(&self) -> Option<&Vec<Document<'v>>> {
        if let Document::Array(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// Returns the array, if Document is an Array, otherwise None
    pub fn to_array(self) -> Option<Vec<Document<'v>>> {
        if let Document::Array(val) = self {
            Some(val)
        } else {
            None
        }
    }

    /// returns true if the Document is an array
    pub fn is_array(&self) -> bool {
        self.as_array().is_some()
    }

    /// Returns the Number, if Document is a Number, otherwise None
    pub fn as_number(&self) -> Option<Number> {
        if let Document::Number(val) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_number(self) -> Option<Number> {
        self.as_number()
    }

    /// returns true if the Document is a number (signed/unsigned int or float)
    pub fn is_number(&self) -> bool {
        self.as_number().is_some()
    }

    /// Returns the positive int, if Document is a positive int, otherwise None
    pub fn as_pos_int(&self) -> Option<u64> {
        if let Document::Number(Number::PosInt(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_pos_int(self) -> Option<u64> {
        self.as_pos_int()
    }

    /// returns true if the Document is a positive integer
    pub fn is_pos_int(&self) -> bool {
        self.as_pos_int().is_some()
    }

    /// Returns the signed int, if Document is a signed int, otherwise None
    pub fn as_int(&self) -> Option<i64> {
        if let Document::Number(Number::NegInt(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_int(self) -> Option<i64> {
        self.as_int()
    }

    /// returns true if the Document is a signed integer
    pub fn is_int(&self) -> bool {
        self.as_int().is_some()
    }

    /// Returns the float value, if Document is a float value, otherwise None
    pub fn as_float(&self) -> Option<f64> {
        if let Document::Number(Number::Float(val)) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_float(self) -> Option<f64> {
        (&self).as_float()
    }

    /// returns true if the Document is a float
    pub fn is_float(&self) -> bool {
        self.as_float().is_some()
    }

    /// Returns the bool value, if Document is a bool value, otherwise None
    pub fn as_bool(&self) -> Option<bool> {
        if let Document::Bool(val) = self {
            Some(*val)
        } else {
            None
        }
    }

    pub fn to_bool(self) -> Option<bool> {
        self.as_bool()
    }

    /// returns true if the Document is a boolean
    pub fn is_bool(&self) -> bool {
        self.as_bool().is_some()
    }

    /// Returns borrowed str, if Document is a string value, otherwise None
    pub fn as_str(&self) -> Option<&str> {
        if let Document::String(val) = self {
            Some(val.as_ref())
        } else {
            None
        }
    }

    /// returns owned String, if Document is a String value
    pub fn to_string(self) -> Option<String> {
        if let Document::String(val) = self {
            Some(val.to_string())
        } else {
            None
        }
    }

    /// returns true if the Document is a string type
    pub fn is_str(&self) -> bool {
        self.as_str().is_some()
    }

    /// returns true if the Document is null
    pub fn is_null(&self) -> bool {
        matches!(self, Document::Null)
    }

    pub fn from_null() -> Self {
        Document::Null
    }
}

/// A number type that implements Javascript / JSON semantics, modeled on serde_json:
/// <https://docs.serde.rs/src/serde_json/number.rs.html#20-22>
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    /// Unsigned 64-bit integer value
    PosInt(u64),
    /// Signed 64-bit integer value
    NegInt(i64),
    /// 64-bit floating-point value
    Float(f64),
}

macro_rules! to_num_fn {
    ($name:ident, $typ:ident) => {
        /// Converts to a `$typ`. This conversion may be lossy.
        pub fn $name(self) -> $typ {
            match self {
                Number::PosInt(val) => val as $typ,
                Number::NegInt(val) => val as $typ,
                Number::Float(val) => val as $typ,
            }
        }
    };
}

impl Number {
    to_num_fn!(to_f32, f32);

    to_num_fn!(to_f64, f64);

    to_num_fn!(to_i8, i8);

    to_num_fn!(to_i16, i16);

    to_num_fn!(to_i32, i32);

    to_num_fn!(to_i64, i64);

    to_num_fn!(to_u8, u8);

    to_num_fn!(to_u16, u16);

    to_num_fn!(to_u32, u32);

    to_num_fn!(to_u64, u64);
}

macro_rules! from_num_fn {
    ($typ:ident, $nt:ident) => {
        impl<'v> From<$typ> for Document<'v> {
            fn from(val: $typ) -> Document<'v> {
                Document::Number(Number::$nt(val.into()))
            }
        }
    };
}

from_num_fn!(u64, PosInt);
from_num_fn!(u32, PosInt);
from_num_fn!(u16, PosInt);
from_num_fn!(u8, PosInt);
from_num_fn!(i64, NegInt);
from_num_fn!(i32, NegInt);
from_num_fn!(i16, NegInt);
from_num_fn!(i8, NegInt);
from_num_fn!(f64, Float);
from_num_fn!(f32, Float);

impl<'v> From<std::collections::HashMap<String, Document<'v>>> for Document<'v> {
    fn from(val: HashMap<String, Document<'v>>) -> Self {
        Document::Object(val)
    }
}

impl<'v> From<Vec<u8>> for Document<'v> {
    fn from(val: Vec<u8>) -> Self {
        Document::Blob(Cow::Owned(val))
    }
}

impl<'v> From<&'v [u8]> for Document<'v> {
    fn from(val: &'v [u8]) -> Self {
        Document::Blob(Cow::Borrowed(val))
    }
}

impl<'v> From<Vec<Document<'v>>> for Document<'v> {
    fn from(val: Vec<Document<'v>>) -> Self {
        Document::Array(val)
    }
}

impl<'v> From<String> for Document<'v> {
    fn from(val: String) -> Self {
        Document::String(Cow::Owned(val))
    }
}

impl<'v> From<&'v str> for Document<'v> {
    fn from(val: &'v str) -> Self {
        Document::String(Cow::Borrowed(val))
    }
}
