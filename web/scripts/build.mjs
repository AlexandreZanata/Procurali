// Minimal native web build: tsc ES modules plus a tracked copy step.
// No bundler, no runtime framework. Fails loudly when the compiler fails
// or a required output is missing.
import { execFile } from "node:child_process";
import { copyFile, mkdir, readdir, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const srcDir = path.join(root, "src");
const publicDir = path.join(root, "public");
const outDir = path.join(root, "dist");
const requiredOutputs = ["main.js", "base.css"];

function run(command, args, cwd) {
  return new Promise((resolve, reject) => {
    execFile(command, args, { cwd }, (error, stdout, stderr) => {
      if (error) {
        process.stderr.write(stdout);
        process.stderr.write(stderr);
        reject(error);
        return;
      }
      resolve({ stdout, stderr });
    });
  });
}

async function exists(file) {
  try {
    await stat(file);
    return true;
  } catch {
    return false;
  }
}

async function build() {
  await mkdir(outDir, { recursive: true });
  await run("npx", ["tsc"], root);
  const entries = await readdir(publicDir);
  for (const entry of entries) {
    const from = path.join(publicDir, entry);
    if ((await stat(from)).isFile()) {
      await copyFile(from, path.join(outDir, entry));
    }
  }
  // The existing server-rendered template ships alongside the bundle so a
  // fresh checkout serves the same public page with the same styles.
  const template = path.join(publicDir, "request-template.html");
  if (await exists(template)) {
    await copyFile(template, path.join(outDir, "request-template.html"));
  }
  const missing = [];
  for (const output of requiredOutputs) {
    if (!(await exists(path.join(outDir, output)))) {
      missing.push(output);
    }
  }
  if (missing.length > 0) {
    throw new Error(`missing required output: ${missing.join(", ")}`);
  }
  // Browser-correct specifier check: emitted entry must not contain
  // extensionless relative imports (browsers refuse them).
  const { readFile } = await import("node:fs/promises");
  const entry = await readFile(path.join(outDir, "main.js"), "utf8");
  const bad = [...entry.matchAll(/from\s+["'](\.[^"']*)["']/g)]
    .map((match) => match[1])
    .filter((specifier) => !specifier.endsWith(".js") && !specifier.startsWith("http"));
  if (bad.length > 0) {
    throw new Error(`extensionless browser import: ${bad.join(", ")}`);
  }
}

const selfTest = process.argv.includes("--self-test");

try {
  if (!(await exists(path.join(srcDir, "main.ts")))) {
    throw new Error("missing required input: src/main.ts");
  }
  await build();
  console.log(`build: OK (${requiredOutputs.join(", ")})`);
  if (selfTest) {
    console.log("self-test: required outputs present, specifiers browser-correct");
  }
} catch (error) {
  console.error(`build: FAILED (${error.message})`);
  process.exit(1);
}
