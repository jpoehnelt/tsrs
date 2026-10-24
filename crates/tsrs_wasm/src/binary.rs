// Embedded protocol v1: bounded typed values and raw binary replies. No JSON text or base64.
use super::{RequestExtension, call, open_session, session};
use std::cell::RefCell;
use tsrs_api::{
    ApiError, ApiResult, Response,
    wire::{Obj, Params},
};
use tsrs_core::{
    collections::OrderedMap,
    json::{JsonMap, Value},
};
const MAGIC: &[u8] = b"TSRB\x01";
const LIMIT: usize = 64 * 1024 * 1024;
const DEPTH: usize = 128;
#[derive(Clone)]
enum Reply {
    Value(Value),
    Bytes(Vec<u8>),
    Array(Vec<Reply>),
    Object(Vec<(String, Reply)>),
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    buffers: Vec<&'a [u8]>,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> ApiResult<&'a [u8]> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| ApiError::invalid_request("truncated binary frame"))?;
        let result = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(result)
    }
    fn length(&mut self) -> ApiResult<usize> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()) as usize)
    }
    fn string(&mut self) -> ApiResult<String> {
        let n = self.length()?;
        std::str::from_utf8(self.take(n)?)
            .map(str::to_owned)
            .map_err(|_| ApiError::invalid_request("invalid UTF8 string"))
    }
    fn value(&mut self, depth: usize) -> ApiResult<Value> {
        if depth > DEPTH {
            return Err(ApiError::invalid_request("binary nesting limit"));
        }
        Ok(match self.take(1)?[0] {
            0 => Value::Null,
            1 => Value::Bool(false),
            2 => Value::Bool(true),
            3 => {
                let n = f64::from_le_bytes(self.take(8)?.try_into().unwrap());
                if !n.is_finite() {
                    return Err(ApiError::invalid_request("non-finite binary number"));
                }
                Value::Number(n)
            }
            4 => Value::String(self.string()?),
            5 => {
                let n = self.length()?;
                if n > self.bytes.len() - self.offset {
                    return Err(ApiError::invalid_request("invalid array length"));
                }
                let mut out = Vec::with_capacity(n);
                for _ in 0..n {
                    out.push(self.value(depth + 1)?);
                }
                Value::Array(out)
            }
            6 => {
                let n = self.length()?;
                if n > (self.bytes.len() - self.offset) / 5 {
                    return Err(ApiError::invalid_request("invalid map length"));
                }
                let mut out = JsonMap::new();
                for _ in 0..n {
                    let key = self.string()?;
                    if out.contains_key(&key) {
                        return Err(ApiError::invalid_request("duplicate binary key"));
                    }
                    out.insert(key, self.value(depth + 1)?);
                }
                Value::Object(out)
            }
            9 => {
                let n = self.length()?;
                let bytes = self.take(n)?;
                let index = self.buffers.len();
                self.buffers.push(bytes);
                Obj::new().set("$raw", Value::Number(index as f64)).build()
            }
            8 => Value::Integer(i64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            _ => return Err(ApiError::invalid_request("unknown binary tag")),
        })
    }
}
fn length(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&u32::try_from(n).expect("bounded embedded frame").to_le_bytes());
}
fn string(out: &mut Vec<u8>, s: &str) {
    length(out, s.len());
    out.extend_from_slice(s.as_bytes());
}
fn value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => out.push(0),
        Value::Bool(false) => out.push(1),
        Value::Bool(true) => out.push(2),
        Value::Number(n) => {
            out.push(3);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::Integer(n) => {
            out.push(8);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::String(s) => {
            out.push(4);
            string(out, s);
        }
        Value::Array(items) => {
            out.push(5);
            length(out, items.len());
            for item in items {
                value(out, item);
            }
        }
        Value::Object(items) => {
            out.push(6);
            length(out, items.len());
            for (key, item) in items {
                string(out, key);
                value(out, item);
            }
        }
    }
}
fn reply(out: &mut Vec<u8>, v: &Reply) {
    match v {
        Reply::Value(v) => value(out, v),
        Reply::Bytes(bytes) => {
            out.push(9);
            length(out, bytes.len());
            out.extend_from_slice(bytes);
        }
        Reply::Array(items) => {
            out.push(5);
            length(out, items.len());
            for item in items {
                reply(out, item);
            }
        }
        Reply::Object(items) => {
            out.push(6);
            length(out, items.len());
            for (key, item) in items {
                string(out, key);
                reply(out, item);
            }
        }
    }
}
thread_local! {static PAGES: RefCell<(u64,OrderedMap<String,Vec<Reply>>)> = RefCell::new((0,OrderedMap::default()));}
fn batch(
    params: &Value,
    extension: Option<RequestExtension>,
    buffers: &[&[u8]],
) -> ApiResult<Reply> {
    let p = Params(params);
    p.object()?;
    let token = p.str("continuationToken")?;
    let mut rows = if token.is_empty() {
        let mut rows = Vec::new();
        if let Value::Array(requests) = p.get("requests") {
            for request in requests {
                let method = Params(request).str("method")?.to_owned();
                let answer = if method == "batchRequests" {
                    Err(ApiError::invalid_request("batchRequests cannot be nested"))
                } else {
                    invoke(request, extension, buffers)
                };
                let mut row = vec![("method".into(), Reply::Value(Value::String(method)))];
                match answer {
                    Ok(result) => row.push(("result".into(), result)),
                    Err(error) => {
                        row.push(("result".into(), Reply::Value(Value::Null)));
                        row.push(("error".into(), Reply::Value(Value::String(error.to_string()))));
                    }
                }
                rows.push(Reply::Object(row));
            }
        } else if !matches!(p.get("requests"), Value::Null) {
            return Err(ApiError::invalid_request("requests must be an array"));
        }
        rows
    } else {
        PAGES
            .with_borrow_mut(|(_, pages)| pages.shift_remove(token))
            .ok_or_else(|| ApiError::client("invalid continuation token"))?
    };
    let max = match p.get("maxResponseBytesPerPage") {
        Value::Number(n) => *n as i64,
        Value::Integer(n) => *n,
        Value::Null => 0,
        _ => return Err(ApiError::invalid_request("invalid page size")),
    };
    let max = if max <= 0 { 64 * 1024 } else { usize::try_from(max).unwrap_or(LIMIT).min(LIMIT) };
    let mut size = 0;
    let mut count = 0;
    for row in &rows {
        let mut encoded = Vec::new();
        reply(&mut encoded, row);
        if count > 0 && size + encoded.len() > max {
            break;
        }
        size += encoded.len();
        count += 1;
    }
    let remaining = rows.split_off(count);
    let mut fields = vec![("responses".into(), Reply::Array(rows))];
    if !remaining.is_empty() {
        let token = PAGES.with_borrow_mut(|(sequence, pages)| {
            *sequence += 1;
            let token = format!("embedded-{}", *sequence);
            pages.insert(token.clone(), remaining);
            token
        });
        fields.push(("continuationToken".into(), Reply::Value(Value::String(token))));
    }
    Ok(Reply::Object(fields))
}
fn invoke(
    request: &Value,
    extension: Option<RequestExtension>,
    buffers: &[&[u8]],
) -> ApiResult<Reply> {
    let p = Params(request);
    p.object()?;
    let method = p.str("method")?;
    let params = p.get("params");
    if method == "openSession" {
        return open_session(true).map(Reply::Value);
    }
    if method == "closeSession" {
        PAGES.with_borrow_mut(|pages| *pages = (0, OrderedMap::default()));
        return call(request, extension).map(Reply::Value);
    }
    let s = session()?;
    if let Some(answer) = extension.and_then(|handler| handler(&s, method, params)) {
        return answer.map(Reply::Value);
    }
    if ["supportedCodeFixes", "collectIdle", "editor", "textDocument/hover"].contains(&method) {
        return call(request, None).map(Reply::Value);
    }
    if method == "ping" {
        return Ok(Reply::Value(Value::String("pong".into())));
    }
    if method == "echo" {
        return Ok(Reply::Value(params.clone()));
    }
    if method == "batchRequests" {
        return batch(params, extension, buffers);
    }
    if matches!(method, "printNode" | "formatNodeForInsertion")
        && let Value::Object(raw) = Params(params).get("data")
        && let Some(Value::Number(index)) = raw.get("$raw")
    {
        let data = buffers
            .get(*index as usize)
            .ok_or_else(|| ApiError::invalid_request("invalid raw buffer"))?;
        return match s.handle_native_binary_request(method, params, data)? {
            Response::Value(answer) => Ok(Reply::Value(answer)),
            _ => Err(ApiError::internal("unexpected printing response")),
        };
    }
    if matches!(method, "printNode" | "formatNodeForInsertion") {
        return Err(ApiError::invalid_request("embedded printing requires a raw AST buffer"));
    }
    match tsrs_api::with_compact_diagnostics(|| s.handle_native_request(method, params))? {
        Response::Value(value) => Ok(Reply::Value(value)),
        Response::Binary(bytes) => Ok(Reply::Bytes(bytes)),
        Response::Json(_) => Err(ApiError::internal("serialized response reached native dispatch")),
    }
}
/// Decode one bounded request and encode one response; extension calls borrow the live session.
pub fn dispatch(bytes: &[u8], extension: Option<RequestExtension>) -> Vec<u8> {
    let answer = (|| {
        if bytes.len() > LIMIT || !bytes.starts_with(MAGIC) {
            return Err(ApiError::invalid_request("invalid binary frame"));
        }
        let mut reader = Reader { bytes, offset: MAGIC.len(), buffers: Vec::new() };
        let request = reader.value(0)?;
        if reader.offset != bytes.len() {
            return Err(ApiError::invalid_request("trailing binary bytes"));
        }
        invoke(&request, extension, &reader.buffers)
    })();
    let mut out = MAGIC.to_vec();
    let envelope = match answer {
        Ok(result) => Reply::Object(vec![("result".into(), result)]),
        Err(error) => {
            Reply::Value(Obj::new().set("error", Value::String(error.to_string())).build())
        }
    };
    reply(&mut out, &envelope);
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_frames_reject_without_panicking() {
        for bytes in
            [b"".as_slice(), b"TSRB\x01\x04\xff\xff\xff\xff", b"TSRB\x01\x05\xff\xff\xff\xff"]
        {
            let response = dispatch(bytes, None);
            let mut reader = Reader { bytes: &response, offset: MAGIC.len(), buffers: Vec::new() };
            assert_ne!(Params(&reader.value(0).unwrap()).str("error").unwrap(), "");
        }
    }
    fn send(request: &Value) -> Reply {
        let mut bytes = MAGIC.to_vec();
        value(&mut bytes, request);
        let out = dispatch(&bytes, None);
        let mut r = Reader { bytes: &out, offset: MAGIC.len(), buffers: vec![] };
        let response = r.value(0).unwrap();
        assert!(matches!(Params(&response).get("error"), Value::Null), "{response:?}");
        Reply::Value(Params(&response).get("result").clone())
    }
    #[test]
    fn binary_sessions_recover_and_batch_errors_are_independent() {
        send(&Obj::new().set("method", Value::String("openSession".into())).build());
        assert!(dispatch(b"bad", None).starts_with(MAGIC));
        let request = Obj::new().set("method", Value::String("ping".into())).build();
        let Reply::Value(pong) = send(&request) else { panic!("not a value") };
        assert_eq!(pong, Value::String("pong".into()));
        let requests =
            vec![request, Obj::new().set("method", Value::String("notAMethod".into())).build()];
        let Reply::Value(result) = send(
            &Obj::new()
                .set("method", Value::String("batchRequests".into()))
                .set("params", Obj::new().set("requests", Value::Array(requests)).build())
                .build(),
        ) else {
            panic!("not a value")
        };
        let Value::Array(rows) = Params(&result).get("responses") else { panic!("missing rows") };
        assert_eq!(rows.len(), 2);
        assert_eq!(Params(&rows[0]).get("result"), &Value::String("pong".into()));
        assert_ne!(Params(&rows[1]).str("error").unwrap(), "");
        send(&Obj::new().set("method", Value::String("closeSession".into())).build());
    }
    #[test]
    fn native_integer_handles_preserve_range() {
        let fields = Obj::new().set("snapshot", Value::Integer(i64::MAX)).build();
        assert_eq!(Params(&fields).u64("snapshot").unwrap(), i64::MAX as u64);
        assert!(
            Params(&Obj::new().set("snapshot", Value::Integer(-1)).build())
                .u64("snapshot")
                .is_err()
        );
    }
    #[test]
    fn typed_scalar_roundtrip() {
        let original = Obj::new()
            .set("name", Value::String("😀".into()))
            .set("integer", Value::Integer(i64::MAX))
            .set("items", Value::Array(vec![Value::Null, Value::Bool(true)]))
            .build();
        let mut bytes = Vec::new();
        value(&mut bytes, &original);
        let mut r = Reader { bytes: &bytes, offset: 0, buffers: Vec::new() };
        assert_eq!(r.value(0).unwrap(), original);
    }
}
