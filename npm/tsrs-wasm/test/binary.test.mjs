import assert from 'node:assert/strict';
import test from 'node:test';
import {encodeFrame,decodeFrame} from '../binary.js';
test('typed frames preserve unicode, scalars and owned raw buffers',()=>{
 const input={text:'é🧪',items:[null,true,false,3.5],data:new Uint8Array([0,255,128]),__proto__:null};
 const frame=encodeFrame(input);const output=decodeFrame(frame);
 assert.deepEqual(output.text,input.text);assert.deepEqual(output.items,input.items);assert.deepEqual(output.data,input.data);
 frame.fill(0);assert.deepEqual(output.data,input.data);
 const dangerous=Object.defineProperty({},'__proto__',{value:{polluted:true},enumerable:true});
 const result=decodeFrame(encodeFrame(dangerous));assert.equal(Object.getPrototypeOf(result),Object.prototype);assert.equal(Object.hasOwn(result,'__proto__'),true);assert.equal({}.polluted,undefined);
});
test('invalid, oversized, cyclic and deeply nested frames fail explicitly',()=>{
 const bytes=encodeFrame({a:1});
 for(let end=0;end<bytes.length;end++) assert.throws(()=>decodeFrame(bytes.subarray(0,end)));
 assert.throws(()=>decodeFrame(new Uint8Array([...bytes,0])),/Trailing/);
 const cycle={};cycle.self=cycle;assert.throws(()=>encodeFrame(cycle),/Cyclic/);
 let deep=null;for(let i=0;i<130;i++)deep=[deep];assert.throws(()=>encodeFrame(deep),/nesting/);
 assert.throws(()=>encodeFrame('\ud800'),/surrogate/);
 assert.throws(()=>encodeFrame(new Uint8Array(64*1024*1024)),/64 MiB/);
});
