// The agent's IPC binding for an AppPort application on disk.
//
// Hosts the project's own application behind `AppPortConnection` (the duplex
// connection @appport/server uses for WebSocket, IPC and in-process transports)
// and carries one AppPort envelope per line over stdin/stdout. Nothing here
// knows about the application: every request goes through the runtime's own
// pipeline (validate, authorize, execute, validate output), with a session
// holding only the permissions the agent's plan derived from the contract.
//
// usage: node appport-ipc-host.mjs <project root> <module> <export> <permissions JSON>

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

const [root, modulePath, exportName, permissionsJson] = process.argv.slice(2);

function fail(message) {
  process.stderr.write(`appport-ipc-host: ${message}\n`);
  process.exit(2);
}

// Resolves an ESM-only package the way the project itself would see it.
function resolvePackage(from, name) {
  for (let dir = resolve(from); ; dir = dirname(dir)) {
    const manifest = join(dir, "node_modules", name, "package.json");
    if (existsSync(manifest)) {
      const pkg = JSON.parse(readFileSync(manifest, "utf8"));
      const entry = pkg.exports?.["."]?.import ?? pkg.exports?.["."]?.default ?? pkg.module ?? pkg.main ?? "index.js";
      return pathToFileURL(join(dirname(manifest), entry)).href;
    }
    if (dirname(dir) === dir) fail(`${name} is not installed for ${from} (run npm install there)`);
  }
}

const { AppPortConnection } = await import(resolvePackage(root, "@appport/server"));
const exported = (await import(pathToFileURL(resolve(root, modulePath)).href))[exportName];
if (exported === undefined) fail(`${modulePath} does not export ${exportName}`);
const application = typeof exported === "function" ? await exported() : exported;
if (typeof application?.handleRequest !== "function") fail(`${modulePath}#${exportName} is not an AppPort application`);

const principal = { id: "appport-agent", type: "service", displayName: "appport-agent verification session" };
const session = {
  id: "appport-agent-session",
  principal,
  applicationId: application.descriptor.id,
  createdAt: new Date().toISOString(),
  permissions: JSON.parse(permissionsJson ?? "[]")
};

let pending = Promise.resolve();
const connection = new AppPortConnection({
  application,
  transport: "ipc",
  session,
  principal,
  send: (envelope) => {
    process.stdout.write(`${JSON.stringify(envelope)}\n`);
  }
});

const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
lines.on("line", (line) => {
  if (line.trim() === "") return;
  // Envelopes are handled in arrival order, like a single IPC channel.
  pending = pending.then(() => connection.accept(line)).catch((error) => {
    process.stderr.write(`appport-ipc-host: ${error?.stack ?? error}\n`);
  });
});
lines.on("close", async () => {
  await pending;
  connection.close();
  await application.close();
  process.exit(0);
});
