import { mockIPC } from "@tauri-apps/api/mocks";
import enLocaleToml from "../src-tauri/locales/en-US.toml?raw";
import jaLocaleToml from "../src-tauri/locales/ja-JP.toml?raw";

type LocaleBundle = {
  locale: string;
  messages: Record<string, string>;
};

type ProfileDetail = {
  id: string;
  name: string;
  note: string;
  path: string;
  iconPath: string | null;
  group: string;
  runtime: Record<string, unknown>;
  pipeline: Record<string, unknown>;
};

const localeBundles: Record<string, LocaleBundle> = {
  "ja-JP": { locale: "ja-JP", messages: parseFlatTomlMessages(jaLocaleToml) },
  "en-US": { locale: "en-US", messages: parseFlatTomlMessages(enLocaleToml) },
};

let profiles: ProfileDetail[] = [
  {
    id: "mediapipe-native",
    name: "MediaPipe Native",
    note: "Webcam to UNMF/Z and VMC/UDP",
    path: "profiles/mediapipe-native.toml",
    iconPath: null,
    group: "Main",
    runtime: {
      fps: 60,
      vmcEnabled: true,
      vmcTargetAddr: "127.0.0.1:39539",
      zenohEnabled: true,
      zenohKeyExpr: "un-motion/frame",
      zenohTopicMode: "frame",
      zenohStreamId: null,
      zenohProducer: "un-motion-capturer",
      engine: "mediapipe-native",
      device: "cam0:Preview Camera",
      resolution: "1280x720",
      mediaPipeRunningMode: "live_stream",
      mediaPipeHolisticEnabled: true,
      vmcReceiveListenAddr: "0.0.0.0:39539",
      ifacialmocapReceiveListenAddr: "0.0.0.0:49983",
      modifierHeadEnabled: true,
      modifierFaceEnabled: true,
      modifierHandsEnabled: true,
      modifierArmsIkEnabled: true,
      modifierTorsoEnabled: true,
      modifierLegsEnabled: false,
      modifierFeetEnabled: false,
      modifierMirrorMode: "normal",
      modifierSmoothingPreset: "medium",
      modifierSmoothingEmaEnabled: true,
      modifierSmoothingEmaAlpha: 0.45,
      modifierSmoothingOneEuroEnabled: false,
      modifierSmoothingConfidenceAdaptiveCutoff: false,
      modifierAdaptiveMinCutoffHz: 0.35,
      modifierAdaptiveBeta: 0.08,
      modifierAdaptiveDerivativeCutoffHz: 1.0,
      modifierHoldLostLandmarks: true,
      modifierEaseRecovery: true,
      modifierLimitRotationJumps: true,
      modifierHeadSourceSwitchBlend: true,
      modifierLostSignalBehavior: "rest-pose",
      modifierLostSignalRestPoseBlend: 0.3,
      modifierLostSignalHoldSeconds: 8.2,
      modifierLostSignalHeadBehavior: "hold",
      modifierLostSignalHeadRestPoseBlend: 0.3,
      modifierLostSignalHeadHoldSeconds: 8.2,
      modifierLostSignalHandsBehavior: "rest-pose",
      modifierLostSignalHandsRestPoseBlend: 0.3,
      modifierLostSignalHandsHoldSeconds: 8.2,
      modifierLostSignalArmsBehavior: "rest-pose",
      modifierLostSignalArmsRestPoseBlend: 0.3,
      modifierLostSignalArmsHoldSeconds: 8.2,
      modifierLostSignalRecoverySeconds: 0.25,
    },
    pipeline: {
      input: "webcam-mediafoundation",
      postProcess: "mediapipe-holistic-v1",
      inputPath: null,
      inputFps: 60,
      inputWidth: null,
      inputHeight: null,
      inputRepeat: false,
      inputFfmpegPath: null,
      inputResizeEnabled: false,
      inputResizeAxis: null,
      inputResizeReference: null,
      inputResizeWidth: null,
      inputResizeHeight: null,
      inputResizePreserveAspect: true,
      inputResizePadColor: null,
    },
  },
  {
    id: "vmc-receive",
    name: "VMC Receive",
    note: "OSC/UDP receive, publish as UNMotionFrame",
    path: "profiles/vmc-receive.toml",
    iconPath: null,
    group: "Inputs",
    runtime: {
      engine: "vmc",
      fps: 60,
      vmcEnabled: false,
      vmcTargetAddr: "127.0.0.1:39539",
      zenohEnabled: true,
      zenohKeyExpr: "un-motion/frame",
      zenohTopicMode: "by-stream-id",
      zenohStreamId: "vmc-receive",
      zenohProducer: "un-motion-capturer",
      vmcReceiveListenAddr: "0.0.0.0:39539",
      ifacialmocapReceiveListenAddr: "0.0.0.0:49983",
      modifierHeadEnabled: true,
      modifierFaceEnabled: true,
      modifierHandsEnabled: true,
      modifierArmsIkEnabled: true,
      modifierTorsoEnabled: true,
      modifierLegsEnabled: true,
      modifierFeetEnabled: true,
      modifierMirrorMode: "normal",
      modifierSmoothingPreset: "off",
      modifierSmoothingEmaEnabled: false,
      modifierSmoothingEmaAlpha: 0.45,
      modifierSmoothingOneEuroEnabled: false,
      modifierSmoothingConfidenceAdaptiveCutoff: false,
      modifierAdaptiveMinCutoffHz: 0.35,
      modifierAdaptiveBeta: 0.08,
      modifierAdaptiveDerivativeCutoffHz: 1.0,
      modifierHoldLostLandmarks: true,
      modifierEaseRecovery: true,
      modifierLimitRotationJumps: true,
      modifierHeadSourceSwitchBlend: true,
      modifierLostSignalBehavior: "hold",
      modifierLostSignalRestPoseBlend: 0.3,
      modifierLostSignalHoldSeconds: 8.2,
      modifierLostSignalHeadBehavior: "hold",
      modifierLostSignalHeadRestPoseBlend: 0.3,
      modifierLostSignalHeadHoldSeconds: 8.2,
      modifierLostSignalHandsBehavior: "hold",
      modifierLostSignalHandsRestPoseBlend: 0.3,
      modifierLostSignalHandsHoldSeconds: 8.2,
      modifierLostSignalArmsBehavior: "hold",
      modifierLostSignalArmsRestPoseBlend: 0.3,
      modifierLostSignalArmsHoldSeconds: 8.2,
      modifierLostSignalRecoverySeconds: 0.25,
    },
    pipeline: {
      engine: "vmc",
      input: "vmc",
      postProcess: null,
      inputPath: null,
      inputFps: null,
      inputWidth: null,
      inputHeight: null,
      inputRepeat: false,
    },
  },
];

let activeProfileId = profiles[0].id;

const appSettings = {
  stopCapturersOnExit: true,
  systemTrayEnabled: false,
  minimizeToTray: true,
  closeToTrayWhileRunning: true,
  startMinimizedToTray: false,
  jumpToCapturersOnQuickLaunch: false,
  themeMode: "system",
  consoleWindowX: null,
  consoleWindowY: null,
  consoleWindowWidth: null,
  consoleWindowHeight: null,
  apiWorkerThreads: 2,
  locale: "ja-JP",
};

export function installDevIpcMock(): void {
  if (hasTauriRuntime()) return;

  mockIPC((cmd, payload) => {
    const args = (payload ?? {}) as Record<string, unknown>;
    switch (cmd) {
      case "i18n_available_locales":
        return Object.keys(localeBundles);
      case "i18n_get_svelte_bundle": {
        const tag = String(args.locale ?? "ja-JP");
        return localeBundles[tag] ?? localeBundles["ja-JP"];
      }
      case "i18n_resolve_default_locale":
        return appSettings.locale;
      case "app_version":
        return "dev-preview";
      case "get_app_settings":
      case "sync_app_settings":
        return appSettings;
      case "list_capturers":
        return [];
      case "list_profiles":
        return profiles.map((profile) => ({
          id: profile.id,
          name: profile.name,
          note: profile.note,
          iconPath: profile.iconPath,
          group: profile.group,
          engine: profile.runtime.engine,
        }));
      case "reorder_profiles": {
        const ids = (args.profileIds as string[]) ?? [];
        profiles = ids
          .map((id) => profiles.find((profile) => profile.id === id))
          .filter((profile): profile is ProfileDetail => Boolean(profile))
          .concat(profiles.filter((profile) => !ids.includes(profile.id)));
        return profiles.map((profile) => ({
          id: profile.id,
          name: profile.name,
          note: profile.note,
          iconPath: profile.iconPath,
          group: profile.group,
          engine: profile.runtime.engine,
        }));
      }
      case "active_profile_id":
        return activeProfileId;
      case "set_active_profile":
        activeProfileId = String(args.profileId ?? activeProfileId);
        return activeProfileId;
      case "get_profile_detail":
        return findProfile(args.profileId);
      case "update_profile_field":
        return findProfile(args.profileId);
      case "create_profile":
      case "duplicate_profile":
        return profiles[0];
      case "delete_profile":
      case "stop_all_capturers":
      case "stop_capturer":
      case "open_external_url":
      case "save_supervisor_logs":
      case "pick_file_path":
      case "reveal_profiles_dir":
      case "reveal_supervisor_logs_dir":
        return null;
      case "launch_capturer":
        return null;
      case "capturer_runtime_status":
        return null;
      case "enumerate_webcams":
        return [
          { id: "cam0:Preview Camera", label: "Preview Camera" },
          { id: "cam1:Virtual Camera", label: "Virtual Camera" },
        ];
      case "enumerate_webcam_formats":
        return [
          { width: 1280, height: 720, fps: 60, pixelFormat: "NV12", label: "1280x720 @ 60 NV12" },
          { width: 1920, height: 1080, fps: 30, pixelFormat: "MJPG", label: "1920x1080 @ 30 MJPG" },
        ];
      default:
        throw new Error(`dev IPC mock: unsupported command ${cmd}`);
    }
  });
}

function findProfile(profileId: unknown): ProfileDetail {
  const id = String(profileId ?? activeProfileId);
  return profiles.find((profile) => profile.id === id) ?? profiles[0];
}

function hasTauriRuntime(): boolean {
  return typeof window !== "undefined" && Boolean((window as any).__TAURI_INTERNALS__);
}

function parseFlatTomlMessages(text: string): Record<string, string> {
  const messages: Record<string, string> = {};
  let section = "";

  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#") || line.startsWith("_")) continue;
    const sectionMatch = line.match(/^\[([^\]]+)\]$/);
    if (sectionMatch) {
      section = sectionMatch[1] ?? "";
      continue;
    }
    const pairMatch = line.match(/^([A-Za-z0-9_]+)\s*=\s*("(?:\\.|[^"\\])*")\s*$/);
    if (!pairMatch) continue;
    const key = pairMatch[1];
    const value = JSON.parse(pairMatch[2]) as string;
    messages[section ? `${section}.${key}` : key] = value.replaceAll("%{", "{");
  }

  return messages;
}
