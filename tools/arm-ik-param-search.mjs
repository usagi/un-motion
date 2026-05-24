#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";

const positionalArgs = process.argv.slice(2).filter((arg) => !arg.startsWith("--"));
const captureDir = positionalArgs[0] ?? "target/vmc-captures";
const manifestPathArg = positionalArgs[1];
const generatedWebPoseDir = positionalArgs[2] ?? path.join(captureDir, "runs", "generated-static", "web-pose");
const dataset = [];

for (const sample of loadDataset(captureDir)) {
  const signalFile = latestSignalFile(generatedWebPoseDir, sample.id, sample.fov)
    ?? latestSignalFile(generatedWebPoseDir, `pose${sample.pose}`, sample.fov)
    ?? latestSignalFile(captureDir, sample.id, sample.fov)
    ?? latestSignalFile(captureDir, `pose${sample.pose}`, sample.fov);
  if (!fs.existsSync(sample.expectedPath) || !signalFile) {
    continue;
  }
  dataset.push({
    pose: sample.pose,
    id: sample.id,
    expected: readVmcBones(sample.expectedPath),
    signals: readSignals(signalFile),
  });
}

if (dataset.length === 0) {
  console.error(`no usable static pose dataset in ${captureDir}`);
  process.exit(2);
}

function loadDataset(rootDir) {
  const manifestPath = manifestPathArg ?? path.join(rootDir, "datasets", "static-hands-v1", "manifest.json");
  if (fs.existsSync(manifestPath)) {
    const manifestDir = path.dirname(manifestPath);
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
    return manifest.items.map((item) => ({
      pose: item.id.replace(/^pose/i, ""),
      id: item.id,
      fov: Number(item.fov),
      expectedPath: resolveDataPath(manifestDir, item.warudoFrame),
    }));
  }

  return [
    ["pose1", 120],
    ["pose2", 100],
    ["pose3", 120],
    ["pose4", 120],
    ["pose4m", 120],
    ["pose5", 145],
    ["pose6", 115],
  ].map(([id, fov]) => ({
    pose: id.replace(/^pose/i, ""),
    id,
    fov,
    expectedPath: path.join(rootDir, `vmc-frame-${id}-fov${fov}-39550.jsonl`),
  }));
}

function resolveDataPath(baseDir, value) {
  return path.isAbsolute(value) ? value : path.join(baseDir, value);
}

const rows = [];
const baselineParams = {
  shoulderX: 0.3,
  shoulderY: 0.17,
  shoulderZ: 0.02,
  swivelX: 0,
  swivelY: -0.55,
  swivelZ: -0.3,
  handPushZ: 0,
};
let baseline = null;
baseline = scoreParams(baselineParams);

if (process.argv.includes("--cross-right")) {
  const crossRows = [];
  for (const shoulderX of [0.22, 0.26, 0.30, 0.34]) {
    for (const shoulderY of [0.12, 0.17, 0.22]) {
      for (const shoulderZ of [-0.08, -0.02, 0.02, 0.08, 0.14]) {
        for (const swivelX of [-0.5, -0.25, 0.0, 0.25, 0.5]) {
          for (const swivelY of [-1.0, -0.65, -0.35, 0.0, 0.35]) {
            for (const swivelZ of [-0.9, -0.55, -0.25, 0.0, 0.35, 0.7]) {
              for (const handPushZ of [-0.35, -0.2, -0.05, 0.1, 0.25, 0.4]) {
                const params = {
                  ...baselineParams,
                  crossRight: { shoulderX, shoulderY, shoulderZ, swivelX, swivelY, swivelZ, handPushZ },
                };
                const row = scoreParams(params);
                if (row) {
                  const pose2Right = scoreOneSide(2, "right", params);
                  crossRows.push({ ...row, pose2Right });
                }
              }
            }
          }
        }
      }
    }
  }
  console.log(`dataset=${dataset.map((sample) => sample.id).join(",")} samples=${dataset.length}`);
  console.log("baseline");
  printRow(baseline);
  console.log("");
  console.log("cross-right pose2 candidates");
  crossRows
    .filter((row) => row.pose2Right)
    .sort((left, right) => left.pose2Right.score - right.pose2Right.score)
    .slice(0, 30)
    .forEach((row) => {
      const p = row.params.crossRight;
      console.log(
        `pose2R=${row.pose2Right.score.toFixed(1)} ` +
        `upper=${row.pose2Right.upper.toFixed(1)} lower=${row.pose2Right.lower.toFixed(1)} hand=${row.pose2Right.hand.toFixed(1)} ` +
        `overall=${row.score.toFixed(1)} max=${row.max.toFixed(1)} hand=${row.handMean.toFixed(1)}/${row.handMax.toFixed(1)} ` +
        `shoulder=${p.shoulderX},${p.shoulderY},${p.shoulderZ} swivel=${p.swivelX},${p.swivelY},${p.swivelZ} pushZ=${p.handPushZ}`,
      );
    });
  process.exit(0);
}

for (const shoulderY of [0.14, 0.17, 0.2, 0.23]) {
  for (const shoulderZ of [-0.04, 0.0, 0.02, 0.06, 0.1]) {
    for (const shoulderX of [0.30, 0.34, 0.38]) {
      for (const swivelX of [0.0, 0.15, 0.25, 0.35]) {
        for (const swivelY of [-1.0, -0.75, -0.55, -0.35]) {
          for (const swivelZ of [-0.8, -0.55, -0.3, 0.0, 0.3]) {
            for (const handPushZ of [-0.45, -0.3, -0.15, 0.0, 0.15, 0.3]) {
              scoreParams({ shoulderX, shoulderY, shoulderZ, swivelX, swivelY, swivelZ, handPushZ });
            }
          }
        }
      }
    }
  }
}

console.log(`dataset=${dataset.map((sample) => sample.id).join(",")} samples=${dataset.length}`);
console.log("baseline");
printRow(baseline);

rows.sort((left, right) => left.score - right.score);
printGroup("overall", rows.slice(0, 20));

const handSafeRows = rows
  .filter((row) => row.handMean <= baseline.handMean + 3 && row.handMax <= baseline.handMax + 8)
  .sort((left, right) => left.armScore - right.armScore);
printGroup("hand-safe arm", handSafeRows.slice(0, 20));

const balancedRows = [...rows].sort((left, right) => left.balancedScore - right.balancedScore);
printGroup("balanced", balancedRows.slice(0, 20));

function scoreParams(params) {
  const errors = [];
  const upperErrors = [];
  const lowerErrors = [];
  const handErrors = [];
  const poseRows = [];
  for (const sample of dataset) {
    const poseErrors = [];
    for (const side of ["left", "right"]) {
      const sideName = capitalize(side);
      const result = solveCandidate(sample.signals, side, params);
      if (!result) {
        continue;
      }
      const upperError = quatAngleDeg(sample.expected[`${sideName}UpperArm`], result.upperLocal);
      const lowerError = quatAngleDeg(sample.expected[`${sideName}LowerArm`], result.lowerLocal);
      const handError = quatAngleDeg(sample.expected[`${sideName}Hand`], result.handLocal);
      errors.push(upperError, lowerError, handError);
      upperErrors.push(upperError);
      lowerErrors.push(lowerError);
      handErrors.push(handError);
      poseErrors.push(upperError, lowerError, handError);
    }
    poseRows.push({
      pose: sample.pose,
      mean: average(poseErrors),
      max: Math.max(...poseErrors),
    });
  }
  if (errors.length === 0) {
    return null;
  }
  errors.sort((left, right) => left - right);
  upperErrors.sort((left, right) => left - right);
  lowerErrors.sort((left, right) => left - right);
  handErrors.sort((left, right) => left - right);
  const mean = average(errors);
  const p95 = errors[Math.min(errors.length - 1, Math.floor(errors.length * 0.95))];
  const max = errors.at(-1);
  const row = {
    params,
    mean,
    p95,
    max,
    upperMean: average(upperErrors),
    lowerMean: average(lowerErrors),
    handMean: average(handErrors),
    upperMax: upperErrors.at(-1),
    lowerMax: lowerErrors.at(-1),
    handMax: handErrors.at(-1),
    poseWorst: poseRows.sort((left, right) => right.max - left.max)[0],
    score: mean + (p95 * 0.35) + (max * 0.15),
  };
  row.armScore = ((row.upperMean + row.lowerMean) / 2) + (Math.max(row.upperMax, row.lowerMax) * 0.25);
  const handMeanBase = baseline?.handMean ?? row.handMean;
  const handMaxBase = baseline?.handMax ?? row.handMax;
  row.balancedScore = row.score + Math.max(0, row.handMean - handMeanBase) * 1.5 + Math.max(0, row.handMax - handMaxBase) * 0.6;
  rows.push(row);
  return row;
}

function scoreOneSide(pose, side, params) {
  const sample = dataset.find((candidate) => String(candidate.pose) === String(pose));
  if (!sample) {
    return null;
  }
  const sideName = capitalize(side);
  const result = solveCandidate(sample.signals, side, params);
  if (!result) {
    return null;
  }
  const upper = quatAngleDeg(sample.expected[`${sideName}UpperArm`], result.upperLocal);
  const lower = quatAngleDeg(sample.expected[`${sideName}LowerArm`], result.lowerLocal);
  const hand = quatAngleDeg(sample.expected[`${sideName}Hand`], result.handLocal);
  return {
    upper,
    lower,
    hand,
    score: (upper + lower + hand) / 3,
  };
}

function printGroup(name, groupRows) {
  console.log("");
  console.log(name);
  for (const row of groupRows) {
    printRow(row);
  }
}

function printRow(row) {
  console.log(
    `score=${row.score.toFixed(1)} arm=${row.armScore.toFixed(1)} balanced=${row.balancedScore.toFixed(1)} ` +
    `mean=${row.mean.toFixed(1)} p95=${row.p95.toFixed(1)} max=${row.max.toFixed(1)} ` +
    `upper=${row.upperMean.toFixed(1)}/${row.upperMax.toFixed(1)} ` +
    `lower=${row.lowerMean.toFixed(1)}/${row.lowerMax.toFixed(1)} ` +
    `hand=${row.handMean.toFixed(1)}/${row.handMax.toFixed(1)} ` +
    `worst=pose${row.poseWorst.pose}/${row.poseWorst.max.toFixed(1)} ` +
    `shoulder=${row.params.shoulderX},${row.params.shoulderY},${row.params.shoulderZ} ` +
    `swivel=${row.params.swivelX},${row.params.swivelY},${row.params.swivelZ} pushZ=${row.params.handPushZ}`,
  );
}

function solveCandidate(signals, side, params) {
  // MediaPipe camera space is mirrored into the VMCP-facing arm signals:
  // the current WebPose stream emits positive X for the left shoulder.
  const sideSign = side === "left" ? 1 : -1;
  const rest = side === "left" ? [-1, 0, 0] : [1, 0, 0];
  const wrist = {
    x: numberSignal(signals, `hand.${side}.wrist.x`),
    y: numberSignal(signals, `hand.${side}.wrist.y`),
    z: numberSignal(signals, `hand.${side}.wrist.z`),
  };
  const crossRight = params.crossRight && side === "right" && wrist.x < -0.02 && wrist.y < 0;
  const effective = crossRight ? params.crossRight : params;
  wrist.z += effective.handPushZ;
  const shoulder = {
    x: sideSign * effective.shoulderX,
    y: effective.shoulderY,
    z: effective.shoulderZ,
  };
  const solved = solveArmIk(sideSign, shoulder, wrist, {
    x: sideSign * effective.swivelX,
    y: effective.swivelY,
    z: effective.swivelZ,
  });
  const upper = normalizePointVector(subPoint(solved.elbow, shoulder));
  const lower = normalizePointVector(subPoint(wrist, solved.elbow));
  let plane = normalize(cross(upper, lower));
  if (!upper || !lower || !plane) {
    return null;
  }
  plane = armPlaneSecondary(signals, side, plane);
  const armSecondary = scale(plane, side === "left" ? 1 : -1);
  const upperGlobal = quatFromBasis(rest, [0, 1, 0], upper, armSecondary);
  const lowerGlobal = quatFromBasis(rest, [0, 1, 0], lower, armSecondary);
  const handGlobal = handGlobalRotation(signals, side, rest);
  if (!upperGlobal || !lowerGlobal || !handGlobal) {
    return null;
  }
  const shoulderLocal = shoulderRotation(side, upper);
  return {
    upperLocal: quatMul(quatInverse(shoulderLocal), upperGlobal),
    lowerLocal: quatMul(quatInverse(upperGlobal), lowerGlobal),
    handLocal: quatMul(quatInverse(lowerGlobal), handGlobal),
  };
}

function handGlobalRotation(signals, side, rest) {
  const forward = vecSignal(signals, `hand.${side}.palm.forward`);
  const across = vecSignal(signals, `hand.${side}.palm.across`);
  const normal = vecSignal(signals, `hand.${side}.palm.normal`);
  const wristX = numberSignal(signals, `hand.${side}.wrist.x`);
  const fingerFold = handFingerFold(signals, side);
  const primary = handPalmPrimary(side, forward, normal);
  const secondary = handPalmSecondary(side, forward, across, normal, wristX, fingerFold);
  return quatFromBasis(rest, [0, 0, 1], primary, secondary);
}

function handPalmPrimary(side, forward, normal) {
  if (side === "right" && validVec3(normal) && forward[0] > 0.75 && normal[2] < 0.25) {
    return forward;
  }
  return scale(forward, -1);
}

function handPalmSecondary(side, forward, across, normal, wristX, fingerFold) {
  if (side === "right" && validVec3(normal) && normal[2] < 0.25) {
    return scale(across, -1);
  }
  if (side === "right" && wristX < -0.02 && normal[2] > 0.25 && normal[2] < 0.9) {
    return across;
  }
  if (side === "right" && isRightFoldedFrontPalm(forward, normal, fingerFold)) {
    return across;
  }
  if (side === "left" && isLeftPointingFrontPalm(forward, normal, fingerFold)) {
    return normal;
  }
  if (side === "left" && normal[0] > 0.9) {
    return scale(across, -1);
  }
  if (side === "left" && normal[2] > 0.7) {
    return scale(normal, -1);
  }
  if (side === "left" && normal[2] > 0.25) {
    return scale(across, -1);
  }
  if (side === "right" && normal[2] > 0.25 && normal[2] < 0.9) {
    return scale(normal, -1);
  }
  return across;
}

function armPlaneSecondary(signals, side, plane) {
  const forward = vecSignal(signals, `hand.${side}.palm.forward`);
  const normal = vecSignal(signals, `hand.${side}.palm.normal`);
  const fingerFold = handFingerFold(signals, side);
  if (side === "left") {
    if (normal[2] < -0.7 && !isLeftPointingFrontPalm(forward, normal, fingerFold)) {
      return mix(plane, [0, 0, -1], 0.25) ?? plane;
    }
    if (normal[2] > 0.7) {
      return mix(plane, normal, 0.35) ?? plane;
    }
  } else if (isRightFoldedFrontPalm(forward, normal, fingerFold)) {
    return mix(plane, [0, 0, 1], 0.25) ?? plane;
  }
  return plane;
}

function isRightFoldedFrontPalm(forward, normal, fingerFold) {
  return normal[2] > 0.75 && normal[2] < 0.95 && forward[2] < -0.2 && fingerFold > 0.25;
}

function isLeftPointingFrontPalm(forward, normal, fingerFold) {
  return normal[2] < -0.75 && forward[2] > 0.1 && fingerFold < 0.15;
}

function handFingerFold(signals, side) {
  return average(["middle", "ring", "little"].map((finger) =>
    numberSignal(signals, `hand.${side}.${finger}.curl`),
  ));
}

function solveArmIk(sideSign, shoulder, wrist, preferred) {
  const upperLen = 0.48;
  const lowerLen = 0.46;
  const shoulderToWrist = subPoint(wrist, shoulder);
  const distance = clamp(lengthPoint(shoulderToWrist), 0.08, upperLen + lowerLen - 0.01);
  const axis = normalizePoint(shoulderToWrist) ?? { x: sideSign, y: 0, z: 0 };
  const along = ((upperLen * upperLen) - (lowerLen * lowerLen) + (distance * distance)) / (2 * distance);
  const height = Math.sqrt(Math.max(0, (upperLen * upperLen) - (along * along)));
  const base = addPoint(shoulder, scalePoint(axis, along));
  const preferredNormal = normalizePoint(preferred) ?? { x: 0, y: -1, z: 0 };
  const plane = normalizePoint(subPoint(preferredNormal, scalePoint(axis, dotPoint(preferredNormal, axis))))
    ?? normalizePoint(crossPoint(axis, { x: 0, y: 1, z: 0 }))
    ?? { x: 0, y: -1, z: 0 };
  return {
    elbow: addPoint(base, scalePoint(plane, height)),
  };
}

function latestSignalFile(dir, sampleId, fov) {
  if (!fs.existsSync(dir)) {
    return null;
  }
  const labels = [
    `static-${sampleId}-fov${fov}`,
    `native-${sampleId}-fov${fov}`,
  ];
  const files = fs.readdirSync(dir)
    .filter((name) => name.endsWith(".json") && labels.some((label) => name.includes(label)))
    .map((name) => ({ name, stat: fs.statSync(path.join(dir, name)) }))
    .sort((left, right) => right.stat.mtimeMs - left.stat.mtimeMs);
  return files[0] ? path.join(dir, files[0].name) : null;
}

function readSignals(file) {
  const json = JSON.parse(fs.readFileSync(file, "utf8"));
  return new Map(json.signals.map((signal) => [signal.name, Number(signal.value)]));
}

function readVmcBones(file) {
  const bones = {};
  for (const line of fs.readFileSync(file, "utf8").split(/\r?\n/)) {
    if (!line.trim()) {
      continue;
    }
    const entry = JSON.parse(line);
    if (entry.addr === "/VMC/Ext/Bone/Pos") {
      bones[entry.args[0].value] = entry.args.slice(4, 8).map((arg) => Number(arg.value));
    }
  }
  return bones;
}

function numberSignal(signals, name) {
  return signals.get(name) ?? 0;
}

function vecSignal(signals, prefix) {
  return ["x", "y", "z"].map((axis) => numberSignal(signals, `${prefix}.${axis}`));
}

function validVec3(vector) {
  return dot(vector, vector) > 0.25;
}

function shoulderRotation(side, upper) {
  const sideSign = side === "left" ? 1 : -1;
  const lift = clamp(upper[1], -1, 1);
  return eulerRadiansToQuat(-0.35, 0, (-0.02 + lift * -0.10) * sideSign);
}

function eulerRadiansToQuat(pitch, yaw, roll) {
  const cy = Math.cos(yaw * 0.5);
  const sy = Math.sin(yaw * 0.5);
  const cp = Math.cos(pitch * 0.5);
  const sp = Math.sin(pitch * 0.5);
  const cr = Math.cos(roll * 0.5);
  const sr = Math.sin(roll * 0.5);
  return normalize([
    (sr * cp * cy) - (cr * sp * sy),
    (cr * sp * cy) + (sr * cp * sy),
    (cr * cp * sy) - (sr * sp * cy),
    (cr * cp * cy) + (sr * sp * sy),
  ]);
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
  const y = normalize(sub(secondary, scale(x, dot(secondary, x))));
  if (!y) {
    return null;
  }
  const z = normalize(cross(x, y));
  return z ? [x, y, z] : null;
}

function quatFromRotationMatrix(matrix) {
  const trace = matrix[0][0] + matrix[1][1] + matrix[2][2];
  let quat;
  if (trace > 0) {
    const scaleValue = Math.sqrt(trace + 1) * 2;
    quat = [
      (matrix[2][1] - matrix[1][2]) / scaleValue,
      (matrix[0][2] - matrix[2][0]) / scaleValue,
      (matrix[1][0] - matrix[0][1]) / scaleValue,
      0.25 * scaleValue,
    ];
  } else if (matrix[0][0] > matrix[1][1] && matrix[0][0] > matrix[2][2]) {
    const scaleValue = Math.sqrt(1 + matrix[0][0] - matrix[1][1] - matrix[2][2]) * 2;
    quat = [
      0.25 * scaleValue,
      (matrix[0][1] + matrix[1][0]) / scaleValue,
      (matrix[0][2] + matrix[2][0]) / scaleValue,
      (matrix[2][1] - matrix[1][2]) / scaleValue,
    ];
  } else if (matrix[1][1] > matrix[2][2]) {
    const scaleValue = Math.sqrt(1 + matrix[1][1] - matrix[0][0] - matrix[2][2]) * 2;
    quat = [
      (matrix[0][1] + matrix[1][0]) / scaleValue,
      0.25 * scaleValue,
      (matrix[1][2] + matrix[2][1]) / scaleValue,
      (matrix[0][2] - matrix[2][0]) / scaleValue,
    ];
  } else {
    const scaleValue = Math.sqrt(1 + matrix[2][2] - matrix[0][0] - matrix[1][1]) * 2;
    quat = [
      (matrix[0][2] + matrix[2][0]) / scaleValue,
      (matrix[1][2] + matrix[2][1]) / scaleValue,
      0.25 * scaleValue,
      (matrix[1][0] - matrix[0][1]) / scaleValue,
    ];
  }
  return normalize(quat);
}

function quatMul(left, right) {
  return [
    (left[3] * right[0]) + (left[0] * right[3]) + (left[1] * right[2]) - (left[2] * right[1]),
    (left[3] * right[1]) - (left[0] * right[2]) + (left[1] * right[3]) + (left[2] * right[0]),
    (left[3] * right[2]) + (left[0] * right[1]) - (left[1] * right[0]) + (left[2] * right[3]),
    (left[3] * right[3]) - (left[0] * right[0]) - (left[1] * right[1]) - (left[2] * right[2]),
  ];
}

function quatInverse(quat) {
  return [-quat[0], -quat[1], -quat[2], quat[3]];
}

function quatAngleDeg(left, right) {
  if (!left || !right) {
    return 180;
  }
  const value = Math.abs(dot(left, right));
  return Math.acos(clamp(value, -1, 1)) * 2 * 180 / Math.PI;
}

function average(values) {
  return values.reduce((sum, value) => sum + value, 0) / Math.max(1, values.length);
}

function capitalize(value) {
  return value[0].toUpperCase() + value.slice(1);
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
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

function addPoint(left, right) {
  return { x: left.x + right.x, y: left.y + right.y, z: left.z + right.z };
}

function subPoint(left, right) {
  return { x: left.x - right.x, y: left.y - right.y, z: left.z - right.z };
}

function scalePoint(point, factor) {
  return { x: point.x * factor, y: point.y * factor, z: point.z * factor };
}

function dotPoint(left, right) {
  return (left.x * right.x) + (left.y * right.y) + (left.z * right.z);
}

function crossPoint(left, right) {
  return {
    x: (left.y * right.z) - (left.z * right.y),
    y: (left.z * right.x) - (left.x * right.z),
    z: (left.x * right.y) - (left.y * right.x),
  };
}

function lengthPoint(point) {
  return Math.hypot(point.x, point.y, point.z);
}

function normalizePoint(point) {
  const length = lengthPoint(point);
  return length > 1e-6 ? scalePoint(point, 1 / length) : null;
}

function normalizePointVector(point) {
  const normalized = normalizePoint(point);
  return normalized ? [normalized.x, normalized.y, normalized.z] : null;
}
