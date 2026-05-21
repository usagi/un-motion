#!/usr/bin/env node
import fs from "node:fs";

const DEFAULT_BONES = [
  "Head",
  "LeftShoulder",
  "RightShoulder",
  "LeftUpperArm",
  "RightUpperArm",
  "LeftLowerArm",
  "RightLowerArm",
  "LeftHand",
  "RightHand",
  "LeftThumbProximal",
  "LeftThumbIntermediate",
  "LeftThumbDistal",
  "LeftIndexProximal",
  "LeftIndexIntermediate",
  "LeftIndexDistal",
  "RightThumbProximal",
  "RightThumbIntermediate",
  "RightThumbDistal",
  "RightIndexProximal",
  "RightIndexIntermediate",
  "RightIndexDistal",
];

function usage() {
  console.error("usage: node tools/vmc-compare.mjs [--mirror-expected] [--corrections-json out.json] expected.jsonl actual.jsonl [BoneName ...]");
  process.exit(2);
}

const { expectedPath, actualPath, selectedBones, correctionsJsonPath, mirrorExpected } = parseArgs(process.argv.slice(2));
if (!expectedPath || !actualPath) {
  usage();
}

const bones = selectedBones.length > 0 ? selectedBones : DEFAULT_BONES;
const expected = readVmcJsonl(expectedPath, bones, { mirror: mirrorExpected });
const actual = readVmcJsonl(actualPath, bones);

console.log(`expected: ${expectedPath}`);
if (mirrorExpected) {
  console.log("expectedMirror: left/right swap + horizontal transform flip");
}
console.log(summary(expected));
console.log(`actual:   ${actualPath}`);
console.log(summary(actual));
console.log("");
console.log("bone,nExpected,nActual,meanDeg,p50Deg,p95Deg,jitterP95Deg,expectedQuat,actualQuat,correctionQuat");

const expectedMeans = new Map();
const actualMeans = new Map();
const correctionRecords = [];
for (const bone of bones) {
  const expectedRows = expected.byBone.get(bone) ?? [];
  const actualRows = actual.byBone.get(bone) ?? [];
  if (expectedRows.length === 0 || actualRows.length === 0) {
    continue;
  }
  const expectedQuat = meanQuat(expectedRows.map((row) => row.quat));
  const actualQuat = meanQuat(actualRows.map((row) => row.quat));
  const correctionQuat = quatMul(expectedQuat, quatInverse(actualQuat));
  const meanDeg = quatAngleDeg(expectedQuat, actualQuat);
  expectedMeans.set(bone, expectedQuat);
  actualMeans.set(bone, actualQuat);
  const actualToExpected = actualRows.map((row) => quatAngleDeg(expectedQuat, row.quat));
  const actualJitter = actualRows.map((row) => quatAngleDeg(actualQuat, row.quat));
  const record = {
    bone,
    nExpected: expectedRows.length,
    nActual: actualRows.length,
    meanDeg,
    p50Deg: quantile(actualToExpected, 0.5),
    p95Deg: quantile(actualToExpected, 0.95),
    jitterP95Deg: quantile(actualJitter, 0.95),
    expectedQuat,
    actualQuat,
    correctionQuat,
  };
  correctionRecords.push(record);
  console.log([
    bone,
    expectedRows.length,
    actualRows.length,
    record.meanDeg.toFixed(2),
    record.p50Deg.toFixed(2),
    record.p95Deg.toFixed(2),
    record.jitterP95Deg.toFixed(2),
    formatQuat(expectedQuat),
    formatQuat(actualQuat),
    formatQuat(correctionQuat),
  ].join(","));
}

console.log("");
console.log("worst actual frames:");
for (const frame of worstFrames(expectedMeans, actual).slice(0, 16)) {
  console.log(`${frame.elapsedMs}ms max=${frame.maxDeg.toFixed(1)} ${frame.parts.map((part) => `${part.bone}:${part.deg.toFixed(1)}`).join(" ")}`);
}

const armChains = armChainRows(expectedMeans, actualMeans);
if (armChains.length > 0) {
  console.log("");
  console.log("arm chain global:");
  for (const row of armChains) {
    console.log(`${row.side} handGlobalDeg=${row.handGlobalDeg.toFixed(1)} lowerGlobalDeg=${row.lowerGlobalDeg.toFixed(1)}`);
  }
}

if (correctionsJsonPath) {
  writeCorrectionsJson(correctionsJsonPath, expectedPath, actualPath, correctionRecords);
  console.log("");
  console.log(`wrote correction candidates: ${correctionsJsonPath}`);
}

function parseArgs(args) {
  let correctionsJsonPath = "";
  let mirrorExpected = false;
  const positional = [];
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--mirror-expected") {
      mirrorExpected = true;
      continue;
    }
    if (arg === "--corrections-json") {
      correctionsJsonPath = args[index + 1] ?? "";
      index += 1;
      if (!correctionsJsonPath) {
        usage();
      }
      continue;
    }
    if (arg === "--help" || arg === "-h") {
      usage();
    }
    positional.push(arg);
  }
  const [expectedPath, actualPath, ...selectedBones] = positional;
  return { expectedPath, actualPath, selectedBones, correctionsJsonPath, mirrorExpected };
}

function writeCorrectionsJson(path, expectedPath, actualPath, records) {
  const payload = {
    schema: "unmotion.vmcCorrectionCandidates.v1",
    generatedAt: new Date().toISOString(),
    expectedPath,
    actualPath,
    // correctionQuat is expected * inverse(actual). If applied directly, use
    // corrected = correctionQuat * actual for the same local bone convention.
    bones: records.map((record) => ({
      bone: record.bone,
      nExpected: record.nExpected,
      nActual: record.nActual,
      meanDeg: round(record.meanDeg),
      p50Deg: round(record.p50Deg),
      p95Deg: round(record.p95Deg),
      jitterP95Deg: round(record.jitterP95Deg),
      expectedQuat: record.expectedQuat.map(round),
      actualQuat: record.actualQuat.map(round),
      correctionQuat: record.correctionQuat.map(round),
    })),
  };
  fs.writeFileSync(path, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}

function readVmcJsonl(path, selected, options = {}) {
  const selectedSet = new Set(selected);
  const byBone = new Map();
  const addrCounts = new Map();
  let lineCount = 0;
  let minTimestamp = Number.POSITIVE_INFINITY;
  let maxTimestamp = 0;

  for (const line of fs.readFileSync(path, "utf8").split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    const entry = JSON.parse(line);
    lineCount += 1;
    minTimestamp = Math.min(minTimestamp, entry.timestampMs);
    maxTimestamp = Math.max(maxTimestamp, entry.timestampMs);
    addrCounts.set(entry.addr, (addrCounts.get(entry.addr) ?? 0) + 1);

    if (entry.addr !== "/VMC/Ext/Bone/Pos") {
      continue;
    }
    let bone = entry.args[0]?.value;
    if (options.mirror) {
      bone = swapLeftRightName(bone);
    }
    if (!selectedSet.has(bone)) {
      continue;
    }
    const values = entry.args.slice(1).map((arg) => Number(arg.value));
    const position = values.slice(0, 3);
    const quat = values.slice(3, 7);
    if (options.mirror) {
      position[0] = -position[0];
      quat[1] = -quat[1];
      quat[2] = -quat[2];
    }
    const row = {
      timestampMs: entry.timestampMs,
      bone,
      position,
      quat,
    };
    if (!byBone.has(bone)) {
      byBone.set(bone, []);
    }
    byBone.get(bone).push(row);
  }

  return { byBone, addrCounts, lineCount, minTimestamp, maxTimestamp };
}

function swapLeftRightName(name) {
  if (typeof name !== "string") {
    return name;
  }
  if (name.startsWith("Left")) {
    return `Right${name.slice("Left".length)}`;
  }
  if (name.startsWith("Right")) {
    return `Left${name.slice("Right".length)}`;
  }
  if (name.startsWith("left")) {
    return `right${name.slice("left".length)}`;
  }
  if (name.startsWith("right")) {
    return `left${name.slice("right".length)}`;
  }
  if (name.endsWith("Left")) {
    return `${name.slice(0, -"Left".length)}Right`;
  }
  if (name.endsWith("Right")) {
    return `${name.slice(0, -"Right".length)}Left`;
  }
  if (name.endsWith("left")) {
    return `${name.slice(0, -"left".length)}right`;
  }
  if (name.endsWith("right")) {
    return `${name.slice(0, -"right".length)}left`;
  }
  return name;
}

function summary(data) {
  const addrs = [...data.addrCounts.entries()]
    .sort((left, right) => right[1] - left[1])
    .slice(0, 8)
    .map(([addr, count]) => `${addr}:${count}`)
    .join(" | ");
  const duration = Number.isFinite(data.minTimestamp) ? data.maxTimestamp - data.minTimestamp : 0;
  return `lines=${data.lineCount} durationMs=${duration} ${addrs}`;
}

function worstFrames(expectedMeans, actual) {
  const startedAt = actual.minTimestamp;
  const byTimestamp = new Map();
  for (const [bone, rows] of actual.byBone.entries()) {
    const expected = expectedMeans.get(bone);
    if (!expected) {
      continue;
    }
    for (const row of rows) {
      if (!byTimestamp.has(row.timestampMs)) {
        byTimestamp.set(row.timestampMs, []);
      }
      byTimestamp.get(row.timestampMs).push({ bone, deg: quatAngleDeg(expected, row.quat) });
    }
  }

  return [...byTimestamp.entries()]
    .map(([timestampMs, parts]) => ({
      elapsedMs: timestampMs - startedAt,
      maxDeg: Math.max(...parts.map((part) => part.deg)),
      parts: parts.sort((left, right) => right.deg - left.deg).slice(0, 6),
    }))
    .sort((left, right) => right.maxDeg - left.maxDeg);
}

function armChainRows(expectedMeans, actualMeans) {
  const rows = [];
  for (const side of ["Left", "Right"]) {
    const expectedUpper = expectedMeans.get(`${side}UpperArm`);
    const expectedLower = expectedMeans.get(`${side}LowerArm`);
    const expectedHand = expectedMeans.get(`${side}Hand`);
    const actualUpper = actualMeans.get(`${side}UpperArm`);
    const actualLower = actualMeans.get(`${side}LowerArm`);
    const actualHand = actualMeans.get(`${side}Hand`);
    if (!expectedUpper || !expectedLower || !expectedHand || !actualUpper || !actualLower || !actualHand) {
      continue;
    }
    const expectedLowerGlobal = quatMul(expectedUpper, expectedLower);
    const actualLowerGlobal = quatMul(actualUpper, actualLower);
    rows.push({
      side,
      lowerGlobalDeg: quatAngleDeg(expectedLowerGlobal, actualLowerGlobal),
      handGlobalDeg: quatAngleDeg(quatMul(expectedLowerGlobal, expectedHand), quatMul(actualLowerGlobal, actualHand)),
    });
  }
  return rows;
}

function meanQuat(quats) {
  const acc = [0, 0, 0, 0];
  for (const quat of quats) {
    const normalized = normalizeQuat(quat);
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
  const a = normalizeQuat(left);
  const b = normalizeQuat(right);
  const dot = Math.min(1, Math.max(-1, Math.abs(a.reduce((sum, value, index) => sum + (value * b[index]), 0))));
  return 2 * Math.acos(dot) * 180 / Math.PI;
}

function quatInverse(quat) {
  const normalized = normalizeQuat(quat);
  return [-normalized[0], -normalized[1], -normalized[2], normalized[3]];
}

function quatMul(left, right) {
  const [ax, ay, az, aw] = normalizeQuat(left);
  const [bx, by, bz, bw] = normalizeQuat(right);
  return normalizeQuat([
    aw * bx + ax * bw + ay * bz - az * by,
    aw * by - ax * bz + ay * bw + az * bx,
    aw * bz + ax * by - ay * bx + az * bw,
    aw * bw - ax * bx - ay * by - az * bz,
  ]);
}

function quantile(values, ratio) {
  if (values.length === 0) {
    return 0;
  }
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.floor((sorted.length - 1) * ratio)))];
}

function formatQuat(quat) {
  return quat.map((value) => value.toFixed(3)).join(" ");
}

function round(value) {
  return Number(value.toFixed(6));
}
