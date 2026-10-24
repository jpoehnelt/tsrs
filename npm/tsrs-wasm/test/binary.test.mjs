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
test('frames own exactly their encoded bytes without spare growth capacity',()=>{
 for(const text of ['small', 'x'.repeat(1024*1024), 'é🧪'.repeat(200000)]){
  const frame=encodeFrame({text,other:'tail'});
  assert.equal(frame.byteOffset,0);
  assert.equal(frame.buffer.byteLength,frame.byteLength);
  assert.equal(decodeFrame(frame).text,text);
 }
 assert.deepEqual([...encodeFrame('é🧪')],[84,83,82,66,1,4,6,0,0,0,195,169,240,159,167,170]);
});
test('deferred strings preserve property evaluation and raw buffer ownership',()=>{
 const data=new Uint8Array([1,2,3]);let reads=0;
 const value={data,text:'é🧪'.repeat(1000),get tail(){if(++reads===2)data.fill(9);return 'tail';}};
 const decoded=decodeFrame(encodeFrame(value));
 assert.deepEqual([...decoded.data],[1,2,3]);assert.equal(reads,2);
 assert.equal(decoded.text,'é🧪'.repeat(1000));assert.equal(decoded.tail,'tail');
});
