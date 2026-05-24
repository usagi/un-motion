#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const dir = process.argv[2] ?? "target/vmc-captures/runs/generated-static/reports";
const files = fs.readdirSync(dir)
  .filter((name) => /^vmc-compare-generated-.+-fov\d+\.txt$/i.test(name))
  .sort((left, right) => left.localeCompare(right, undefined, { numeric: true }));

if (files.length === 0) {
  console.error(`no generated regression reports in ${dir}`);
  process.exit(2);
}

const rows = [];
const chainRows = [];
for (const file of files) {
  const sample = file.match(/^vmc-compare-generated-(.+)-fov\d+\.txt$/i)?.[1].replace(/^static-/, "") ?? "?";
  const text = fs.readFileSync(path.join(dir, file), "utf8");
  for (const line of text.split(/\r?\n/)) {
    if (/^[A-Za-z].*,\d+,\d+,/.test(line)) {
      const [bone, , , meanDeg] = line.split(",");
      rows.push({ sample, bone, meanDeg: Number(meanDeg), file });
      continue;
    }
    const chain = line.match(/^(Left|Right) handGlobalDeg=([\d.]+) lowerGlobalDeg=([\d.]+)/);
    if (chain) {
      chainRows.push({
        sample,
        side: chain[1],
        handGlobalDeg: Number(chain[2]),
        lowerGlobalDeg: Number(chain[3]),
        file,
      });
    }
  }
}

const armBones = new Set(["LeftShoulder", "RightShoulder", "LeftUpperArm", "RightUpperArm", "LeftLowerArm", "RightLowerArm", "LeftHand", "RightHand"]);
const fingerPattern = /(Thumb|Index|Middle|Ring|Little)/;

console.log("worst local bones:");
for (const row of rows.toSorted((left, right) => right.meanDeg - left.meanDeg).slice(0, 24)) {
  console.log(`${row.sample} ${row.bone} ${row.meanDeg.toFixed(1)}deg`);
}

console.log("");
console.log("worst arm/hand local bones:");
for (const row of rows
  .filter((row) => armBones.has(row.bone))
  .toSorted((left, right) => right.meanDeg - left.meanDeg)
  .slice(0, 18)) {
  console.log(`${row.sample} ${row.bone} ${row.meanDeg.toFixed(1)}deg`);
}

console.log("");
console.log("worst fingers:");
for (const row of rows
  .filter((row) => fingerPattern.test(row.bone))
  .toSorted((left, right) => right.meanDeg - left.meanDeg)
  .slice(0, 18)) {
  console.log(`${row.sample} ${row.bone} ${row.meanDeg.toFixed(1)}deg`);
}

console.log("");
console.log("arm chain global:");
for (const row of chainRows.toSorted((left, right) =>
  Math.max(right.handGlobalDeg, right.lowerGlobalDeg) - Math.max(left.handGlobalDeg, left.lowerGlobalDeg),
)) {
  console.log(`${row.sample} ${row.side} hand=${row.handGlobalDeg.toFixed(1)}deg lower=${row.lowerGlobalDeg.toFixed(1)}deg`);
}

console.log("");
console.log("aggregate:");
printStats("local", rows.map((row) => row.meanDeg));
printStats("arm/hand local", rows.filter((row) => armBones.has(row.bone)).map((row) => row.meanDeg));
printStats("finger local", rows.filter((row) => fingerPattern.test(row.bone)).map((row) => row.meanDeg));
printStats("arm chain lower", chainRows.map((row) => row.lowerGlobalDeg));
printStats("arm chain hand", chainRows.map((row) => row.handGlobalDeg));

function printStats(label, values) {
  if (values.length === 0) {
    console.log(`${label}: n=0`);
    return;
  }
  const sorted = [...values].sort((left, right) => left - right);
  const mean = sorted.reduce((sum, value) => sum + value, 0) / sorted.length;
  console.log(
    `${label}: n=${sorted.length} mean=${mean.toFixed(1)}deg ` +
    `p95=${quantile(sorted, 0.95).toFixed(1)}deg max=${sorted.at(-1).toFixed(1)}deg`,
  );
}

function quantile(sortedValues, ratio) {
  return sortedValues[Math.min(sortedValues.length - 1, Math.max(0, Math.floor((sortedValues.length - 1) * ratio)))];
}
