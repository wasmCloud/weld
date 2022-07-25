// This file is @generated by wasmcloud/weld-codegen 0.4.6.
// It is not intended for manual editing.
// namespace: org.wasmcloud.model

#[allow(unused_imports)]
use crate::{
    cbor::{Decoder, Encoder, Write},
    error::{RpcError, RpcResult},
};
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};

#[allow(dead_code)]
pub const SMITHY_VERSION: &str = "1.0";

/// Capability contract id, e.g. 'wasmcloud:httpserver'
/// This declaration supports code generations and is not part of an actor or provider sdk
pub type CapabilityContractId = String;

// Encode CapabilityContractId as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
pub fn encode_capability_contract_id<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &CapabilityContractId,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.str(val)?;
    Ok(())
}

// Decode CapabilityContractId from cbor input stream
#[doc(hidden)]
pub fn decode_capability_contract_id(
    d: &mut crate::cbor::Decoder<'_>,
) -> Result<CapabilityContractId, RpcError> {
    let __result = { d.str()?.to_string() };
    Ok(__result)
}
/// 32-bit float
pub type F32 = f32;

// Encode F32 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_f32<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &F32,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.f32(*val)?;
    Ok(())
}

// Decode F32 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_f32(d: &mut crate::cbor::Decoder<'_>) -> Result<F32, RpcError> {
    let __result = { d.f32()? };
    Ok(__result)
}
/// 64-bit float aka double
pub type F64 = f64;

// Encode F64 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_f64<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &F64,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.f64(*val)?;
    Ok(())
}

// Decode F64 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_f64(d: &mut crate::cbor::Decoder<'_>) -> Result<F64, RpcError> {
    let __result = { d.f64()? };
    Ok(__result)
}
/// signed 16-bit int
pub type I16 = i16;

// Encode I16 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_i16<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &I16,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i16(*val)?;
    Ok(())
}

// Decode I16 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i16(d: &mut crate::cbor::Decoder<'_>) -> Result<I16, RpcError> {
    let __result = { d.i16()? };
    Ok(__result)
}
/// signed 32-bit int
pub type I32 = i32;

// Encode I32 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_i32<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &I32,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i32(*val)?;
    Ok(())
}

// Decode I32 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i32(d: &mut crate::cbor::Decoder<'_>) -> Result<I32, RpcError> {
    let __result = { d.i32()? };
    Ok(__result)
}
/// signed 64-bit int
pub type I64 = i64;

// Encode I64 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_i64<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &I64,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i64(*val)?;
    Ok(())
}

// Decode I64 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i64(d: &mut crate::cbor::Decoder<'_>) -> Result<I64, RpcError> {
    let __result = { d.i64()? };
    Ok(__result)
}
/// signed byte
pub type I8 = i8;

// Encode I8 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_i8<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &I8,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i8(*val)?;
    Ok(())
}

// Decode I8 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_i8(d: &mut crate::cbor::Decoder<'_>) -> Result<I8, RpcError> {
    let __result = { d.i8()? };
    Ok(__result)
}
/// list of identifiers
/// This declaration supports code generations and is not part of an actor or provider sdk
pub type IdentifierList = Vec<String>;

// Encode IdentifierList as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
pub fn encode_identifier_list<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &IdentifierList,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.array(val.len() as u64)?;
    for item in val.iter() {
        e.str(item)?;
    }
    Ok(())
}

// Decode IdentifierList from cbor input stream
#[doc(hidden)]
pub fn decode_identifier_list(
    d: &mut crate::cbor::Decoder<'_>,
) -> Result<IdentifierList, RpcError> {
    let __result = {
        if let Some(n) = d.array()? {
            let mut arr: Vec<String> = Vec::with_capacity(n as usize);
            for _ in 0..(n as usize) {
                arr.push(d.str()?.to_string())
            }
            arr
        } else {
            // indefinite array
            let mut arr: Vec<String> = Vec::new();
            loop {
                match d.datatype() {
                    Err(_) => break,
                    Ok(crate::cbor::Type::Break) => break,
                    Ok(_) => arr.push(d.str()?.to_string()),
                }
            }
            arr
        }
    };
    Ok(__result)
}
/// unsigned 16-bit int
pub type U16 = i16;

// Encode U16 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_u16<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &U16,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i16(*val)?;
    Ok(())
}

// Decode U16 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u16(d: &mut crate::cbor::Decoder<'_>) -> Result<U16, RpcError> {
    let __result = { d.i16()? };
    Ok(__result)
}
/// unsigned 32-bit int
pub type U32 = i32;

// Encode U32 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_u32<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &U32,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i32(*val)?;
    Ok(())
}

// Decode U32 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u32(d: &mut crate::cbor::Decoder<'_>) -> Result<U32, RpcError> {
    let __result = { d.i32()? };
    Ok(__result)
}
/// unsigned 64-bit int
pub type U64 = i64;

// Encode U64 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_u64<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &U64,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i64(*val)?;
    Ok(())
}

// Decode U64 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u64(d: &mut crate::cbor::Decoder<'_>) -> Result<U64, RpcError> {
    let __result = { d.i64()? };
    Ok(__result)
}
/// unsigned byte
pub type U8 = i8;

// Encode U8 as CBOR and append to output stream
#[doc(hidden)]
#[allow(unused_mut)]
#[inline]
pub fn encode_u8<W: crate::cbor::Write>(
    mut e: &mut crate::cbor::Encoder<W>,
    val: &U8,
) -> RpcResult<()>
where
    <W as crate::cbor::Write>::Error: std::fmt::Display,
{
    e.i8(*val)?;
    Ok(())
}

// Decode U8 from cbor input stream
#[doc(hidden)]
#[inline]
pub fn decode_u8(d: &mut crate::cbor::Decoder<'_>) -> Result<U8, RpcError> {
    let __result = { d.i8()? };
    Ok(__result)
}
/// Rust codegen traits
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CodegenRust {
    /// if true, disables deriving 'Default' trait
    #[serde(rename = "noDeriveDefault")]
    #[serde(default)]
    pub no_derive_default: bool,
    /// if true, disables deriving 'Eq' trait
    #[serde(rename = "noDeriveEq")]
    #[serde(default)]
    pub no_derive_eq: bool,
    /// adds `[#non_exhaustive]` attribute to a struct declaration
    #[serde(rename = "nonExhaustive")]
    #[serde(default)]
    pub non_exhaustive: bool,
    /// if true, do not generate code for this item.
    /// This trait can be used if an item needs to be hand-generated
    #[serde(default)]
    pub skip: bool,
}

/// indicates that a trait or class extends one or more bases
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Extends {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<IdentifierList>,
}

/// Field sequence number. A zero-based field number for each member of a structure,
/// to enable deterministic cbor serialization and improve forward and backward compatibility.
/// Although the values are not required to be sequential, gaps are filled with nulls
/// during encoding and so will slightly increase the encoding size.
pub type N = i16;

/// A non-empty string (minimum length 1)
pub type NonEmptyString = String;

/// Rename item(s) in target language.
/// Useful if the item name (operation, or field) conflicts with a keyword in the target language.
/// example: @rename({lang:"python",name:"delete"})
pub type Rename = Vec<RenameItem>;

/// list element of trait @rename. the item name in the target language
/// see '@rename'
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RenameItem {
    /// language
    #[serde(default)]
    pub lang: String,
    /// the name of the structure/operation/field
    #[serde(default)]
    pub name: String,
}

/// Overrides for serializer & deserializer
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Serialization {
    /// (optional setting) Override field name when serializing and deserializing
    /// By default, (when `name` not specified) is the exact declared name without
    /// casing transformations. This setting does not affect the field name
    /// produced in code generation, which is always lanaguage-idiomatic
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// This trait doesn't have any functional impact on codegen. It is simply
/// to document that the defined type is a synonym, and to silence
/// the default validator that prints a notice for synonyms with no traits.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Synonym {}

/// The unsignedInt trait indicates that one of the number types is unsigned
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UnsignedInt {}

/// a protocol defines the semantics
/// of how a client and server communicate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Wasmbus {
    /// indicates this service's operations are handled by an actor (default false)
    #[serde(rename = "actorReceive")]
    #[serde(default)]
    pub actor_receive: bool,
    /// capability id such as "wasmcloud:httpserver"
    /// always required for providerReceive, but optional for actorReceive
    #[serde(rename = "contractId")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_id: Option<CapabilityContractId>,
    /// Binary message protocol version. Defaults to "0" if unset.
    /// Be aware that changing this value can break binary compatibility unless
    /// all users of this interface recompile
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// indicates this service's operations are handled by an provider (default false)
    #[serde(rename = "providerReceive")]
    #[serde(default)]
    pub provider_receive: bool,
}

/// data sent via wasmbus
/// This trait is required for all messages sent via wasmbus
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WasmbusData {}
