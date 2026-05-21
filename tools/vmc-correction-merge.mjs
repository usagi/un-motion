#!/usr/bin/env node
import fs from "node:fs";

function usage() {
  console.error("usage: node tools/vmc-correction-merge.mjs [--out merged.json] candidate1.json candidate2.json ...");
  process.exit(2);
}

const { outPath, inputPaths } = parseArgs(process.argv.slice(2));
if (inputPaths.length === 0) {
  usage();
}

const byBone = new Map();
for (const inputPath of inputPaths) {
  const payload = JSON.parse(fs.readFileSync(inputPath, "utf8"));
  if (payload.schema !== "unmotion.vmcCorrectionCandidates.v1") {
    throw new Error(`unsupported correction candidate schema in ${inputPath}`);
  }
  for (const bone of payload.bones ?? []) {
    if (!Array.isArray(bone.correctionQuat) || bone.correctionQuat.length !== 4) {
      continue;
    }
    if (!byBone.has(bone.bone)) {
      byBone.set(bone.bone, []);
    }
    byBone.get(bone.bone).push({
      source: inputPath,
      meanDeg: Number(bone.meanDeg ?? 0),
      jitterP95Deg: Number(bone.jitterP95Deg ?? 0),
      correctionQuat: bone.correctionQuat.map(Number),
    });
  }
}

const bones = [...byBone.entries()]
  .map(([bone, records]) => summarizeBone(bone, records))
  .sort((left, right) => right.meanDegAvg - left.meanDegAvg);

console.log("bone,samples,meanDegAvg,meanDegMax,correctionSpreadDeg,correctionQuat,stable");
for (const bone of bones) {
  console.log([
    bone.bone,
    bone.samples,
    bone.meanDegAvg.toFixed(2),
    bone.meanDegMax.toFixed(2),
    bone.correctionSpreadDeg.toFixed(2),
    formatQuat(bone.correctionQuat),
    bone.stable ? "yes" : "no",
  ].join(","));
}

if (outPath) {
  const payload = {
    schema: "unmotion.vmcCorrectionMerge.v1",
    generatedAt: new Date().toISOString(),
    inputs: inputPaths,
    bones,
  };
  fs.writeFileSync(outPath, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
  console.log("");
  console.log(`wrote merged corrections: ${outPath}`);
}

function parseArgs(args) {
  let outPath = "";
  const inputPaths = [];
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--out") {
      outPath = args[index + 1] ?? "";
      index += 1;
      if (!outPath) {
        usage();
      }
      continue;
    }
    if (arg === "--help" || arg === "-h") {
      usage();
    }
    inputPaths.push(arg);
  }
  return { outPath, inputPaths };
}

function summarizeBone(bone, records) {
  const correctionQuat = meanQuat(records.map((record) => record.correctionQuat));
  const spreads = records.map((record) => quatAngleDeg(correctionQuat, record.correctionQuat));
  const correctionSpreadDeg = spreads.length > 0 ? Math.max(...spreads) : 0;
  const meanDegAvg = average(records.map((record) => record.meanDeg));
  const meanDegMax = Math.max(...records.map((record) => record.meanDeg));
  const jitterP95Max = Math.max(...records.map((record) => record.jitterP95Deg));
  return {
    bone,
    samples: records.length,
    meanDegAvg: round(meanDegAvg),
    meanDegMax: round(meanDegMax),
    jitterP95Max: round(jitterP95Max),
    correctionSpreadDeg: round(correctionSpreadDeg),
    correctionQuat: correctionQuat.map(round),
    stable: records.length >= 2 && correctionSpreadDeg <= 8 && jitterP95Max <= 5,
  };
}

function meanQuat(quats) {
  const acc = [0, 0, 0, 0];
  let reference = null;
  for (const quat of quats) {
    let normalized = normalizeQuat(quat);
    if (reference && dotQuat(reference, normalized) < 0) {
      normalized = normalized.map((value) => -value);
    }
    reference ??= normalized;
    for (let index = 0; index < 4; index += 1) {
      acc[index] += normalized[index];
    }
  }
  return normalizeQuat(acc);
}

function normalizeQuat(quat) {
  const length = Math.hypot(...quat);
  if (length <= 1e-8) {
    return [0, 0, 0, 1];
  }
  const normalized = quat.map((value) => value / length);
  return normalized[3] < 0 ? normalized.map((value) => -value) : normalized;
}

function quatAngleDeg(left, right) {
  const dot = Math.min(1, Math.max(-1, Math.abs(dotQuat(normalizeQuat(left), normalizeQuat(right)))));
  return 2 * Math.acos(dot) * 180 / Math.PI;
}

function dotQuat(left, right) {
  return left.reduce((sum, value, index) => sum + (value * right[index]), 0);
}

function average(values) {
  return values.length > 0 ? values.reduce((sum, value) => sum + value, 0) / values.length : 0;
}

function formatQuat(quat) {
  return quat.map((value) => value.toFixed(3)).join(" ");
}

function round(value) {
  return Number(value.toFixed(6));
}
