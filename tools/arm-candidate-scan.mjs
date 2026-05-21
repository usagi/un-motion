#!/usr/bin/env node
import fs from "node:fs";

function usage() {
  console.error("usage: node tools/arm-candidate-scan.mjs expected-vmc.jsonl web-pose-frame.json");
  process.exit(2);
}

const [expectedPath, webPosePath] = process.argv.slice(2);
if (!expectedPath || !webPosePath) {
  usage();
}

const expected = readVmcBones(expectedPath);
const webPose = JSON.parse(fs.readFileSync(webPosePath, "utf8"));
const signals = new Map((webPose.signals ?? []).map((signal) => [signal.name, Number(signal.value)]));
if (signals.size === 0) {
  populateSignalsFromRawLandmarks(signals, webPose);
}

console.log(`expected=${expectedPath}`);
console.log(`webPose=${webPosePath}`);
console.log(`sequence=${webPose.sequence} capture=${webPose.captureWidth}x${webPose.captureHeight}@${webPose.captureFps}`);
console.log("");

for (const side of ["left", "right"]) {
  scanSide(side);
}

function scanSide(side) {
  const sideName = side[0].toUpperCase() + side.slice(1);
  const rest = side === "left" ? [-1, 0, 0] : [1, 0, 0];
  const shoulder = vec(`arm.${side}.shoulder`);
  const elbow = vec(`arm.${side}.elbow`);
  const wrist = vec(`arm.${side}.wrist`);
  const upper = normalize(sub(elbow, shoulder));
  const lower = normalize(sub(wrist, elbow));
  if (!upper || !lower) {
    console.log(side);
    console.log("  missing arm direction signals");
    console.log("");
    return;
  }
  const naturalPlane = normalize(cross(upper, lower));
  const forward = vec(`hand.${side}.palm.forward`);
  const across = vec(`hand.${side}.palm.across`);
  const normal = vec(`hand.${side}.palm.normal`);

  const armSecondaries = [
    ["plane", naturalPlane],
    ["-plane", scale(naturalPlane, -1)],
    ["plane/back25", mix(naturalPlane, [0, 0, -1], 0.25)],
    ["plane/back50", mix(naturalPlane, [0, 0, -1], 0.5)],
    ["plane/back75", mix(naturalPlane, [0, 0, -1], 0.75)],
    ["-plane/back25", mix(scale(naturalPlane, -1), [0, 0, -1], 0.25)],
    ["-plane/back50", mix(scale(naturalPlane, -1), [0, 0, -1], 0.5)],
    ["-plane/back75", mix(scale(naturalPlane, -1), [0, 0, -1], 0.75)],
    ["plane/front25", mix(naturalPlane, [0, 0, 1], 0.25)],
    ["plane/front50", mix(naturalPlane, [0, 0, 1], 0.5)],
    ["plane/front75", mix(naturalPlane, [0, 0, 1], 0.75)],
    ["-plane/front25", mix(scale(naturalPlane, -1), [0, 0, 1], 0.25)],
    ["-plane/front50", mix(scale(naturalPlane, -1), [0, 0, 1], 0.5)],
    ["-plane/front75", mix(scale(naturalPlane, -1), [0, 0, 1], 0.75)],
    ["up", [0, 1, 0]],
    ["down", [0, -1, 0]],
    ["front", [0, 0, 1]],
    ["back", [0, 0, -1]],
  ];
  const handPrimary = [
    ["-forward", scale(forward, -1)],
    ["forward", forward],
  ];
  const handSecondaries = [
    ["across", across],
    ["-across", scale(across, -1)],
    ["normal", normal],
    ["-normal", scale(normal, -1)],
  ];

  const rows = [];
  for (const [armSecondaryName, armSecondary] of armSecondaries) {
    const upperGlobal = quatFromBasis(rest, [0, 1, 0], upper, armSecondary);
    const lowerGlobal = quatFromBasis(rest, [0, 1, 0], lower, armSecondary);
    if (!upperGlobal || !lowerGlobal) {
      continue;
    }
    const shoulderLocal = shoulderRotation(side, upper);
    const upperLocal = quatMul(quatInverse(shoulderLocal), upperGlobal);
    const lowerLocal = quatMul(quatInverse(upperGlobal), lowerGlobal);
    for (const [handPrimaryName, handPrimaryVector] of handPrimary) {
      for (const [handSecondaryName, handSecondaryVector] of handSecondaries) {
        const handGlobal = quatFromBasis(rest, [0, 0, 1], handPrimaryVector, handSecondaryVector);
        if (!handGlobal) {
          continue;
        }
        const handLocal = quatMul(quatInverse(lowerGlobal), handGlobal);
        const upperErr = quatAngleDeg(expected[`${sideName}UpperArm`], upperLocal);
        const lowerErr = quatAngleDeg(expected[`${sideName}LowerArm`], lowerLocal);
        const handErr = quatAngleDeg(expected[`${sideName}Hand`], handLocal);
        rows.push({
          armSecondaryName,
          handPrimaryName,
          handSecondaryName,
          upperErr,
          lowerErr,
          handErr,
          avg: (upperErr + lowerErr + handErr) / 3,
        });
      }
    }
  }

  rows.sort((left, right) => left.avg - right.avg);
  console.log(side);
  console.log(`  upper=${formatVec(upper)} lower=${formatVec(lower)} plane=${formatVec(naturalPlane)}`);
  for (const row of rows.slice(0, 12)) {
    console.log(
      `  avg=${row.avg.toFixed(1)} upper=${row.upperErr.toFixed(1)} lower=${row.lowerErr.toFixed(1)} hand=${row.handErr.toFixed(1)} arm=${row.armSecondaryName} hand=${row.handPrimaryName}/${row.handSecondaryName}`,
    );
  }
  console.log("");
}

function populateSignalsFromRawLandmarks(signals, frame) {
  const pose = Array.isArray(frame.poseWorldLandmarks) && frame.poseWorldLandmarks.length >= 17
    ? frame.poseWorldLandmarks
    : frame.poseLandmarks;
  if (Array.isArray(pose) && pose.length >= 17) {
    for (const [side, shoulderIndex, elbowIndex, wristIndex] of [
      ["left", 11, 13, 15],
      ["right", 12, 14, 16],
    ]) {
      const shoulder = posePoint(pose[shoulderIndex], true);
      const elbow = posePoint(pose[elbowIndex], true);
      const wrist = posePoint(pose[wristIndex], true);
      putVec(signals, `arm.${side}.shoulder`, shoulder);
      putVec(signals, `arm.${side}.elbow`, elbow);
      putVec(signals, `arm.${side}.wrist`, wrist);
    }
  }
  for (const hand of frame.hands ?? []) {
    const side = String(hand.handednessLabel ?? "").toLowerCase();
    if (side !== "left" && side !== "right") {
      continue;
    }
    const source = Array.isArray(hand.worldLandmarks) && hand.worldLandmarks.length >= 21
      ? hand.worldLandmarks.map((landmark) => handWorldPoint(landmark))
      : (hand.landmarks ?? []).map((landmark) => ({
          x: landmark.x,
          y: landmark.y,
          z: landmark.z,
        }));
    if (source.length < 21) {
      continue;
    }
    const forward = normalize(sub(source[9], source[0]));
    const across = normalize(sub(source[5], source[17]));
    const normal = forward && across ? normalize(cross(across, forward)) : null;
    if (forward) {
      putVec(signals, `hand.${side}.palm.forward`, applyPalmCorrection("forward", forward));
    }
    if (across) {
      putVec(signals, `hand.${side}.palm.across`, applyPalmCorrection("across", across));
    }
    if (normal) {
      putVec(signals, `hand.${side}.palm.normal`, applyPalmCorrection("normal", normal));
    }
  }
}

function posePoint(landmark, isWorld) {
  if (isWorld) {
    return [-Number(landmark.x), -Number(landmark.y), -Number(landmark.z)];
  }
  return [0.5 - Number(landmark.x), 0.5 - Number(landmark.y), -Number(landmark.z)];
}

function handWorldPoint(landmark) {
  return [Number(landmark.x), -Number(landmark.y), -Number(landmark.z)];
}

function applyPalmCorrection(kind, vector) {
  if (kind === "normal") {
    return [vector[0], -vector[1], -vector[2]];
  }
  return [-vector[0], vector[1], vector[2]];
}

function putVec(signals, prefix, vector) {
  signals.set(`${prefix}.x`, vector[0]);
  signals.set(`${prefix}.y`, vector[1]);
  signals.set(`${prefix}.z`, vector[2]);
}

function shoulderRotation(side, upper) {
  return [0, 0, 0, 1];
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function eulerRadiansToQuat(pitch, yaw, roll) {
  const cy = Math.cos(yaw * 0.5);
  const sy = Math.sin(yaw * 0.5);
  const cp = Math.cos(pitch * 0.5);
  const sp = Math.sin(pitch * 0.5);
  const cr = Math.cos(roll * 0.5);
  const sr = Math.sin(roll * 0.5);
  return normalizeQuat([
    (sr * cp * cy) - (cr * sp * sy),
    (cr * sp * cy) + (sr * cp * sy),
    (cr * cp * sy) - (sr * sp * cy),
    (cr * cp * cy) + (sr * sp * sy),
  ]);
}

function readVmcBones(path) {
  const bones = {};
  for (const line of fs.readFileSync(path, "utf8").split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    const entry = JSON.parse(line);
    if (entry.addr !== "/VMC/Ext/Bone/Pos") {
      continue;
    }
    bones[entry.args[0].value] = entry.args.slice(4, 8).map((arg) => Number(arg.value));
  }
  return bones;
}

function vec(prefix) {
  return [
    signals.get(`${prefix}.x`) ?? 0,
    signals.get(`${prefix}.y`) ?? 0,
    signals.get(`${prefix}.z`) ?? 0,
  ];
}

function formatVec(vector) {
  return vector.map((value) => value.toFixed(3)).join(",");
}

function sub(left, right) {
  return left.map((value, index) => value - right[index]);
}

function scale(vector, factor) {
  return vector.map((value) => value * factor);
}

function mix(left, right, amount) {
  return normalize(left.map((value, index) => value * (1 - amount) + right[index] * amount));
}

function dot(left, right) {
  return left.reduce((sum, value, index) => sum + value * right[index], 0);
}

function cross(left, right) {
  return [
    left[1] * right[2] - left[2] * right[1],
    left[2] * right[0] - left[0] * right[2],
    left[0] * right[1] - left[1] * right[0],
  ];
}

function normalize(vector) {
  const length = Math.hypot(...vector);
  return length > 1e-6 ? vector.map((value) => value / length) : null;
}

function quatFromBasis(fromPrimary, fromSecondary, toPrimary, toSecondary) {
  const from = orthonormalBasis(fromPrimary, fromSecondary);
  const to = orthonormalBasis(toPrimary, toSecondary);
  if (!from || !to) {
    return null;
  }
  const matrix = [0, 1, 2].map((row) => [0, 1, 2].map((column) =>
    to[0][row] * from[0][column] + to[1][row] * from[1][column] + to[2][row] * from[2][column],
  ));
  return quatFromRotationMatrix(matrix);
}

function orthonormalBasis(primary, secondary) {
  const x = normalize(primary);
  if (!x) {
    return null;
  }
  const projected = dot(secondary, x);
  const y = normalize(sub(secondary, scale(x, projected)));
  if (!y) {
    return null;
  }
  const z = normalize(cross(x, y));
  return z ? [x, y, z] : null;
}

function quatFromRotationMatrix(matrix) {
  const trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
  if (trace > 0) {
    const scale = Math.sqrt(trace + 1) * 2;
    return normalizeQuat([
      (matrix[2][1] - matrix[1][2]) / scale,
      (matrix[0][2] - matrix[2][0]) / scale,
      (matrix[1][0] - matrix[0][1]) / scale,
      0.25 * scale,
    ]);
  }
  if (matrix[0][0] > matrix[1][1] && matrix[0][0] > matrix[2][2]) {
    const scale = Math.sqrt(1 + matrix[0][0] - matrix[1][1] - matrix[2][2]) * 2;
    return normalizeQuat([
      0.25 * scale,
      (matrix[0][1] + matrix[1][0]) / scale,
      (matrix[0][2] + matrix[2][0]) / scale,
      (matrix[2][1] - matrix[1][2]) / scale,
    ]);
  }
  if (matrix[1][1] > matrix[2][2]) {
    const scale = Math.sqrt(1 + matrix[1][1] - matrix[0][0] - matrix[2][2]) * 2;
    return normalizeQuat([
      (matrix[0][1] + matrix[1][0]) / scale,
      0.25 * scale,
      (matrix[1][2] + matrix[2][1]) / scale,
      (matrix[0][2] - matrix[2][0]) / scale,
    ]);
  }
  const scale = Math.sqrt(1 + matrix[2][2] - matrix[0][0] - matrix[1][1]) * 2;
  return normalizeQuat([
    (matrix[0][2] + matrix[2][0]) / scale,
    (matrix[1][2] + matrix[2][1]) / scale,
    0.25 * scale,
    (matrix[1][0] - matrix[0][1]) / scale,
  ]);
}

function normalizeQuat(quat) {
  const length = Math.hypot(...quat);
  return length > 1e-6 ? quat.map((value) => value / length) : [0, 0, 0, 1];
}

function quatInverse(quat) {
  return [-quat[0], -quat[1], -quat[2], quat[3]];
}

function quatMul(left, right) {
  return [
    left[3] * right[0] + left[0] * right[3] + left[1] * right[2] - left[2] * right[1],
    left[3] * right[1] - left[0] * right[2] + left[1] * right[3] + left[2] * right[0],
    left[3] * right[2] + left[0] * right[1] - left[1] * right[0] + left[2] * right[3],
    left[3] * right[3] - left[0] * right[0] - left[1] * right[1] - left[2] * right[2],
  ];
}

function quatAngleDeg(left, right) {
  if (!left || !right) {
    return Number.POSITIVE_INFINITY;
  }
  const a = normalizeQuat(left);
  const b = normalizeQuat(right);
  const amount = Math.abs(dot(a, b));
  return 2 * Math.acos(Math.min(1, amount)) * 180 / Math.PI;
}
