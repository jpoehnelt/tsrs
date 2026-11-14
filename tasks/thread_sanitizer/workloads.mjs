#!/usr/bin/env node
// Project generation and interactive workloads for the ThreadSanitizer task.
import fs from "node:fs";
import path from "node:path";
import { json, run, write, writeJSON, requireValue, main } from "../../tools/lib/cli.mjs";
import { WatchSession } from "../../tools/watch/session.mjs";
import { Server, fileURI, languageID, offsetToPosition } from "../../tools/lib/lsp.mjs";
main(async () => {
    const [mode, ...a] = process.argv.slice(2);
    if (!mode || ["--help", "-h"].includes(mode)) {
        console.log(
            "Usage: workloads project <catalog> <name> <work> | graph <output> | watch <files...> -- <command...> | select-file <project> | lsp <binary> <project> <file> <log>",
        );
        return;
    }
    if (mode === "project") {
        const [catalog, name, work] = a,
            p = json(catalog).projects.find((p) => p.name === name);
        requireValue(p, `unknown project ${name}`);
        const sub = p.suite_dir ? "suite/" + p.suite_dir : "solutions/" + p.name;
        console.log(work ? path.resolve(work, sub, p.project) : sub + "\n" + p.project);
    } else if (mode === "graph") {
        requireValue(a[0], "output directory is required");
        const root = path.resolve(a[0]),
            n = 12;
        for (let i = 0; i < n; i++) {
            const deps = Array.from({ length: i }, (_, j) => j).filter(
                (j) => j >= i - 3 && ((i + j) % 2 === 0 || j === i - 1),
            );
            writeJSON(path.join(root, `p${i}/tsconfig.json`), {
                compilerOptions: {
                    composite: true,
                    outDir: "out",
                    rootDir: "src",
                    strict: true,
                    target: "es2020",
                    module: "esnext",
                    moduleResolution: "bundler",
                    types: [],
                },
                include: ["src"],
                references: deps.map((j) => ({ path: `../p${j}` })),
            });
            for (let f = 0; f < 40; f++) {
                const imports =
                        f === 0
                            ? deps.map((j) => `import { v${j}_0 } from "../../p${j}/src/f0";\n`).join("")
                            : `import { v${i}_${f - 1} } from "./f${f - 1}";\n`,
                    use = f === 0 ? deps.map((j) => `v${j}_0`).join(" + ") || "0" : `v${i}_${f - 1}`,
                    body = Array.from(
                        { length: 15 },
                        (_, k) =>
                            `export interface S${k} { a: number; b: string; c: S${k}[] }\nexport function g${k}(x: S${k}): number { return x.a + x.c.length; }`,
                    ).join("\n");
                write(
                    path.join(root, `p${i}/src/f${f}.ts`),
                    `${imports}${body}\nexport const v${i}_${f}: number = ${use};\n`,
                );
            }
        }
        writeJSON(path.join(root, "tsconfig.json"), {
            files: [],
            references: Array.from({ length: n }, (_, i) => ({ path: `p${i}` })),
        });
    } else if (mode === "watch") {
        const sep = a.indexOf("--");
        requireValue(sep > 0 && a[sep + 1], "files and a command after -- are required");
        const files = a.slice(0, sep).map((f) => path.resolve(f)),
            cmd = a.slice(sep + 1);
        let cwd = path.dirname(files[0]);
        while (!files.every((f) => f.startsWith(cwd + path.sep))) cwd = path.dirname(cwd);
        const session = new WatchSession(cmd[0], cwd, cmd.slice(1), true);
        try {
            await session.cycles({ timeout: 600000 });
            for (let i = 0; i < files.length; i++) {
                const rel = path.relative(session.project, path.resolve(files[i])),
                    text = session.remember(rel);
                session.edit(rel, () => text + `\nexport const tsanEdit${i} = ${i};\n`);
                await session.cycles({ timeout: 600000 });
                session.edit(rel, () => text);
                await session.cycles({ timeout: 600000 });
            }
        } finally {
            await session.close();
        }
        requireValue(
            [0, 1].includes(session.proc.exitCode) || session.proc.signalCode === "SIGINT",
            `watch compiler exited ${session.proc.exitCode ?? session.proc.signalCode}`,
        );
    } else if (mode === "select-file") {
        const [project] = a;
        requireValue(project, "project directory is required");
        const files = run(["git", "ls-files", "-z", "--", "*.ts"], { cwd: project }).stdout.split("\0");
        const file = files.find(
            (file) => file && !file.endsWith(".d.ts") && /^([ \t]+)return\b/m.test(fs.readFileSync(path.join(project, file), "utf8")),
        );
        requireValue(file, "no tracked TypeScript file with a return statement inside a function body found");
        console.log(JSON.stringify(file));
    } else if (mode === "lsp") {
        const [binary, project, file, log] = a;
        requireValue(binary && project && file && log, "binary, project, file and log are required");
        await lsp(binary, project, file, log);
    } else throw new Error(`unknown workload ${mode}`);
});

async function lsp(binary, project, file, log) {
    const text = fs.readFileSync(path.join(project, file), "utf8");
    const match = text.match(/^([ \t]+)return\b/m);
    requireValue(match, "no return statement inside a function body found");
    const at = match.index + match[1].length;
    const identifiers = [...text.slice(0, at).matchAll(/\b[A-Za-z_]\w{3,}\b/g)].map((match) => match.index);
    const hover = identifiers[Math.floor(identifiers.length / 2)] ?? 0;
    const uri = fileURI(path.join(project, file));
    const server = new Server("tsrs", [binary, "--lsp", "-stdio"], project, log);
    const request = async (method, params) => {
        const response = await server.request(method, params);
        requireValue(!response.error, `${method}: ${JSON.stringify(response.error)}`);
    };
    try {
        await request("initialize", {
            processId: null,
            rootUri: fileURI(project),
            workspaceFolders: [{ uri: fileURI(project), name: path.basename(project) }],
            capabilities: {
                textDocument: {
                    diagnostic: { dynamicRegistration: false },
                    hover: { contentFormat: ["markdown", "plaintext"] },
                    completion: {
                        completionItem: {
                            snippetSupport: true,
                            resolveSupport: { properties: ["documentation", "detail", "additionalTextEdits"] },
                        },
                    },
                },
                workspace: { configuration: true, workspaceFolders: true },
                general: { positionEncodings: ["utf-16"] },
            },
        });
        server.notify("initialized", {});
        server.notify("textDocument/didOpen", {
            textDocument: { uri, languageId: languageID(file), version: 1, text },
        });
        await request("textDocument/diagnostic", { textDocument: { uri } });
        const typed = "let zz = 1; ";
        let current = text,
            n = 0,
            direction = 1;
        for (let i = 1; i <= 40; i++) {
            const start = at + n - (direction < 0 ? 1 : 0);
            const end = start + (direction < 0 ? 1 : 0);
            const inserted = direction > 0 ? typed[n] : "";
            const change = {
                range: { start: offsetToPosition(current, start), end: offsetToPosition(current, end) },
                text: inserted,
            };
            current = current.slice(0, start) + inserted + current.slice(end);
            n += direction;
            if (n === typed.length || n === 0) direction *= -1;
            server.notify("textDocument/didChange", {
                textDocument: { uri, version: i + 1 },
                contentChanges: [change],
            });
            for (const method of ["diagnostic", "hover", "completion"]) {
                await request(`textDocument/${method}`, {
                    textDocument: { uri },
                    ...(method === "diagnostic" ? {} : {
                        position: offsetToPosition(current, method === "completion" ? at + n : hover),
                    }),
                });
            }
        }
        await request("shutdown", null);
        server.notify("exit", undefined);
        let timer;
        try {
            await Promise.race([
                server.done,
                new Promise((_, reject) => {
                    timer = setTimeout(() => reject(new Error("language server did not exit after shutdown")), 10000);
                }),
            ]);
        } finally {
            clearTimeout(timer);
        }
    } finally {
        await server.close();
    }
    // The LSP exit notification cancels the server context and the CLI reports that cancellation with exit 1.
    const canceled = server.proc.exitCode === 1 && server.error.trim() === "context canceled";
    requireValue(server.proc.exitCode === 0 || canceled, `language server exited ${server.proc.exitCode ?? server.proc.signalCode}`);
}
