// Supplementary single-thread embedded transport workload. Use release artifacts and /usr/bin/time -l.
import assert from 'node:assert/strict';
import {readFileSync,writeFileSync} from 'node:fs';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const [wasmPath,hostPath,mode,output]=process.argv.slice(2);
const host=await import(pathToFileURL(hostPath));
const bytes=readFileSync(wasmPath),module=new WebAssembly.Module(bytes);
const create=mode==='binary' ? host.createBinarySessionSync : host.createSessionSync;
const hash=(value)=>createHash('sha256').update(value).digest('hex');
const source='/* é🧪 */'+Array.from({length:6000},(_,i)=>`function f${i}(value){return value};`).join('');
const config=JSON.stringify({compilerOptions:{allowJs:true,checkJs:true,strict:true,noEmit:true,types:[],lib:['esnext']},files:['Input.js']});
const samples=[];
for(let round=0;round<5;round++){
 globalThis.gc();const session=create(module,host.memoryFileSystem());session.request('openSession');
 try{
  const opened=session.request('createSnapshot',{openProjects:['/tsconfig.json'],fileSystem:{kind:'layer',files:{'/tsconfig.json':config,'/Input.js':source}}});
  const base={snapshot:opened.snapshot,project:opened.projects.find(p=>p.configFileName==='/tsconfig.json').id};
  const execute=()=>{
   const diagnostics=session.request('getSemanticDiagnostics',{...base,files:['/Input.js']});
   assert.equal(diagnostics.length,6000);assert.ok(diagnostics.every(d=>d.code===7006));
   const raw=session.request('getSourceFile',{...base,file:'/Input.js'});
   const ast=mode==='binary'?raw:Buffer.from(raw.data,'base64');assert.ok(ast instanceof Uint8Array);
   return {diagnosticsSha256:hash(JSON.stringify(diagnostics)),astSha256:hash(ast),astBytes:ast.length};
  };
  const expected=execute(),times=[];
  for(let repeat=0;repeat<3;repeat++){const start=performance.now();const actual=execute();times.push(performance.now()-start);assert.deepEqual(actual,expected);}
  samples.push({round,...expected,times});
 }finally{session.request('closeSession');}
}
writeFileSync(output,JSON.stringify({kind:'Supplementary sequential embedded transport stress, not a CLI or paired Chrome measurement',compilerSha256:hash(bytes),scriptSha256:hash(readFileSync(new URL(import.meta.url))),sourceSha256:hash(source),samples},null,2)+'\n');
