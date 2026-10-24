// Versioned embedded frames. Binary results are copied out of reactor-owned memory.
const magic = [84, 83, 82, 66, 1];
const limit = 64 * 1024 * 1024;
const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
export function encodeFrame(value) {
    // Capture large immutable strings without allocating temporary UTF-8 bytes.
    // Small scalar writes use bounded chunks; the final frame owns exactly its size.
    let bytes = new Uint8Array(1024), view = new DataView(bytes.buffer);
    const chunks = [];
    let at = 0, size = 0;
    const reserve = (n) => {
        if (size + at + n > limit) throw new Error('Binary frame exceeds 64 MiB');
        if (at + n > bytes.length) {
            chunks.push(bytes.subarray(0, at)); size += at;
            bytes = new Uint8Array(Math.max(1024, n));
            view = new DataView(bytes.buffer); at = 0;
        }
    };
    const byte = (n) => { reserve(1); bytes[at++] = n; };
    const length = (n) => { reserve(4); view.setUint32(at, n, true); at += 4; };
    const string = (s) => {
        if (!s.isWellFormed()) throw new Error('Unpaired UTF-16 surrogate');
        let n = s.length;
        if (!/^[\x00-\x7f]*$/u.test(s)) {
            for (let i = 0; i < s.length; i++) {
                const c = s.charCodeAt(i);
                if (c >= 0x80) {
                    n += c < 0x800 ? 1 : 2;
                    if (c >= 0xd800 && c <= 0xdbff) i++;
                }
            }
        }
        length(n);
        if (n >= 1024) {
            if (size + at + n > limit) throw new Error('Binary frame exceeds 64 MiB');
            chunks.push(bytes.subarray(0, at), s); size += at + n;
            bytes = new Uint8Array(1024); view = new DataView(bytes.buffer); at = 0;
            return;
        }
        reserve(n);
        const result = encoder.encodeInto(s, bytes.subarray(at, at + n));
        if (result.read !== s.length || result.written !== n) throw new Error('Invalid UTF-8 size');
        at += n;
    };
    const ancestors = new Set();
    const put = (v, depth) => {
        if (depth > 128) throw new Error('Binary nesting limit');
        if (v === null || v === undefined) return byte(0);
        if (typeof v === 'boolean') return byte(v ? 2 : 1);
        if (typeof v === 'number') {
            if (!Number.isFinite(v)) return byte(0);
            byte(3); reserve(8); view.setFloat64(at, v, true); at += 8; return;
        }
        if (typeof v === 'string') { byte(4); string(v); return; }
        if (v instanceof Uint8Array) { byte(9); length(v.length); reserve(v.length); bytes.set(v, at); at += v.length; return; }
        if (typeof v !== 'object') throw new Error('Unsupported binary value');
        if (ancestors.has(v)) throw new Error('Cyclic binary value');
        ancestors.add(v);
        if (Array.isArray(v)) { byte(5); length(v.length); for (const item of v) put(item, depth + 1); }
        else {
            const keys = Object.keys(v).filter(k => v[k] !== undefined);
            byte(6); length(keys.length); for (const key of keys) { string(key); put(v[key], depth + 1); }
        }
        ancestors.delete(v);
    };
    for (const b of magic) byte(b);
    put(value, 0);
    const result = new Uint8Array(size + at);
    chunks.push(bytes.subarray(0, at));
    let offset = 0;
    for (const chunk of chunks) {
        if (typeof chunk === 'string') {
            const encoded = encoder.encodeInto(chunk, result.subarray(offset));
            if (encoded.read !== chunk.length) throw new Error('Invalid UTF-8 size');
            offset += encoded.written;
        } else { result.set(chunk, offset); offset += chunk.length; }
    }
    return result;
}
export function decodeFrame(bytes) {
    if (bytes.length > limit || magic.some((b, i) => bytes[i] !== b)) throw new Error('Invalid binary frame');
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    let at = magic.length;
    const take = (n) => { if (n > bytes.length - at) throw new Error('Truncated binary frame'); const start = at; at += n; return start; };
    const length = () => view.getUint32(take(4), true);
    const string = () => { const n = length(); const start = take(n); return decoder.decode(bytes.subarray(start, start + n)); };
    const get = (depth) => {
        if (depth > 128) throw new Error('Binary nesting limit');
        switch (bytes[take(1)]) {
            case 0: return null;
            case 1: return false;
            case 2: return true;
            case 3: { const n = view.getFloat64(take(8), true); if (!Number.isFinite(n)) throw new Error('Non-finite binary number'); return n; }
            case 4: return string();
            case 5: { const n = length(); if (n > bytes.length - at) throw new Error('Invalid array length'); const out = []; for (let i = 0; i < n; i++) out.push(get(depth + 1)); return out; }
            case 6: {
                const n = length(); if (n > (bytes.length - at) / 5) throw new Error('Invalid map length'); const out = {};
                for (let i = 0; i < n; i++) { const key = string(); if (Object.hasOwn(out, key)) throw new Error('Duplicate binary key'); const value = get(depth + 1); if (key === "__proto__") Object.defineProperty(out, key, { value, enumerable: true, writable: true, configurable: true }); else out[key] = value; } return out;
            }
            case 8: return Number(view.getBigInt64(take(8), true));
            case 9: { const n = length(); const start = take(n); return bytes.slice(start, start + n); }
            default: throw new Error('Unknown binary tag');
        }
    };
    const result = get(0); if (at !== bytes.length) throw new Error('Trailing binary bytes'); return result;
}
