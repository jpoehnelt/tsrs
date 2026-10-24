// Synthetic native binary API workload. Measure this fresh process with /usr/bin/time -l.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createBinarySessionSync, memoryFileSystem } from '../../npm/tsrs-wasm/core.js';
const module = new WebAssembly.Module(readFileSync(process.argv[2]));
const session = createBinarySessionSync(module, memoryFileSystem());
const source = 'x;\n'.repeat(40_000);
let answerHash;
session.request('openSession');
try {
    const snapshot = session.request('createSnapshot', {
        openProjects: ['/tsconfig.json'],
        fileSystem: { kind: 'layer', files: {
            '/tsconfig.json': JSON.stringify({ compilerOptions: { noLib: true, noEmit: true }, files: ['/code.ts'] }),
            '/code.ts': source,
        } },
    });
    assert.equal(snapshot.projects.length, 1);
    const params = { snapshot: snapshot.snapshot, project: snapshot.projects[0].id, files: ['/code.ts'] };
    for (let query = 0; query < 3; query++) {
        const diagnostics = session.request('getSemanticDiagnostics', params);
        assert.equal(diagnostics.length, 40_000);
        assert.equal(diagnostics.every(diagnostic => diagnostic.code === 2304), true);
        const hash = createHash('sha256').update(JSON.stringify(diagnostics)).digest('hex');
        if (answerHash !== undefined) assert.equal(hash, answerHash);
        answerHash = hash;
    }
    console.log(JSON.stringify({ queries: 3, diagnostics: 40_000, sourceHash: createHash('sha256').update(source).digest('hex'), answerHash }));
} finally {
    session.request('closeSession');
}
