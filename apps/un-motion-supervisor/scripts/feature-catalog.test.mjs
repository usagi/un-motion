import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/lib/featureCatalog.ts", import.meta.url), "utf8");
const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 } });
const { motionFeatures, matchingFeatures, featureAnchor } = await import(
	`data:text/javascript;base64,${Buffer.from(outputText).toString("base64")}`
);

test("technical and Japanese terms discover the same feature, including full-width input", () => {
	assert.deepEqual(
		matchingFeatures("Ｂｅｔａ").map((f) => f.id),
		["smoothing"]
	);
	assert.deepEqual(
		matchingFeatures("なめらかさ").map((f) => f.id),
		["smoothing"]
	);
	assert.deepEqual(
		matchingFeatures("LAN publisher").map((f) => f.id),
		["unmf"]
	);
	assert.equal(matchingFeatures("存在しない機能").length, 0);
});

test("search leaves all input methods discoverable; incompatible input opens the selector", () => {
	const vmc = motionFeatures.find((f) => f.id === "vmc-input");
	assert.equal(featureAnchor(vmc, "mediapipe-native"), "feature-engine");
	assert.equal(featureAnchor(vmc, "vmc"), "feature-source");
	assert.equal(vmc.engine, "vmc");
	assert.ok(matchingFeatures("").some((f) => f.id === "ifacial"));
});

test("all catalog deep links have unique, focusable destinations in the actual editor", async () => {
	const app = await readFile(new URL("../src/App.svelte", import.meta.url), "utf8");
	assert.equal(new Set(motionFeatures.map((f) => f.id)).size, motionFeatures.length);
	for (const anchor of new Set(motionFeatures.map((f) => f.anchor).filter(Boolean))) {
		assert.equal(app.split(`id="${anchor}"`).length - 1, 1, anchor);
		assert.match(app, new RegExp(`id="${anchor}"[^>]*tabindex="-1"`), anchor);
	}
});
