#!/usr/bin/env node
import fs from "node:fs";

if (process.argv.length < 3) {
  console.error("usage: node tools/web-pose-inspect.mjs web-pose-frame.json");
  process.exit(2);
}

const frame = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const signals = new Map(frame.signals.map((signal) => [signal.name, signal]));

console.log(`sequence=${frame.sequence} capture=${frame.captureWidth}x${frame.captureHeight}@${frame.captureFps}`);
console.log(`fps camera=${fmt(frame.cameraCallbackFps)} media=${fmt(frame.cameraMediaFps)} submit=${fmt(frame.submittedFps)}`);

for (const side of ["left", "right"]) {
  const forward = vec(`hand.${side}.palm.forward`);
  const across = vec(`hand.${side}.palm.across`);
  const normal = vec(`hand.${side}.palm.normal`);
  const calculatedNormal = normalize(cross(across, forward));
  const euler = ["pitch", "yaw", "roll"].map((axis) => scalar(`hand.${side}.wrist.${axis}`));
  const curls = ["thumb", "index", "middle", "ring", "little"]
    .map((finger) => `${finger}=${fmt(scalar(`hand.${side}.${finger}.curl`))}`)
    .join(" ");

  console.log("");
  console.log(`${side}`);
  console.log(`  wrist=${vecText(vec(`hand.${side}.wrist`))}`);
  console.log(`  forward=${vecText(forward)} len=${fmt(length(forward))}`);
  console.log(`  across =${vecText(across)} len=${fmt(length(across))}`);
  console.log(`  normal =${vecText(normal)} len=${fmt(length(normal))}`);
  console.log(`  cross  =${vecText(calculatedNormal)} dot=${fmt(dot(calculatedNormal, normal))}`);
  console.log(`  dot f/a=${fmt(dot(forward, across))} f/n=${fmt(dot(forward, normal))} a/n=${fmt(dot(across, normal))}`);
  console.log(`  legacy pitch/yaw/roll=${vecText(euler)}`);
  console.log(`  curls ${curls}`);
}

if (Array.isArray(frame.partDiagnostics)) {
  console.log("");
  console.log("parts");
  for (const part of frame.partDiagnostics) {
    console.log(`  ${part.part} ${part.state} conf=${fmt(part.confidence)} sig=${part.signalCount} age=${Math.round(part.ageMs)}`);
  }
}

function scalar(name) {
  return signals.get(name)?.value ?? Number.NaN;
}

function vec(prefix) {
  return [scalar(`${prefix}.x`), scalar(`${prefix}.y`), scalar(`${prefix}.z`)];
}

function vecText(vector) {
  return vector.map(fmt).join(",");
}

function fmt(value) {
  return Number.isFinite(value) ? value.toFixed(4) : "NaN";
}

function length(vector) {
  return Math.hypot(...vector);
}

function normalize(vector) {
  const magnitude = length(vector);
  return magnitude > 1e-6 ? vector.map((value) => value / magnitude) : [Number.NaN, Number.NaN, Number.NaN];
}

function dot(left, right) {
  return (left[0] * right[0]) + (left[1] * right[1]) + (left[2] * right[2]);
}

function cross(left, right) {
  return [
    (left[1] * right[2]) - (left[2] * right[1]),
    (left[2] * right[0]) - (left[0] * right[2]),
    (left[0] * right[1]) - (left[1] * right[0]),
  ];
}
