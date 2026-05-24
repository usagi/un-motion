import { copyFileSync, mkdirSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const appDir = resolve(here, "..");
const repoRoot = resolve(appDir, "..", "..");
const sourceDir = join(repoRoot, "assets", "sounds");
const targetDir = join(appDir, "public", "sounds");

mkdirSync(targetDir, { recursive: true });

for (const entry of readdirSync(sourceDir, { withFileTypes: true })) {
  if (!entry.isFile()) continue;
  if (!/\.(flac|ogg|wav)$/i.test(entry.name)) continue;
  copyFileSync(join(sourceDir, entry.name), join(targetDir, entry.name));
}
