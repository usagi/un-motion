<!--
  UN Motion — Supervisor Console（Capturers / Profiles / Logs / Settings）。
  レイアウトは UN Avatar Supervisor と揃えたシェル規約に従う。
-->
<script lang="ts">
  import { convertFileSrc, invoke } from "@tauri-apps/api/core";
  import { flip } from "svelte/animate";
  import { onDestroy, onMount } from "svelte";
  import { _ } from "svelte-i18n";
  import { setUiLocale } from "@usagi.network/un-i18n-svelte";
  import {
    Activity,
    AlertTriangle,
    Copy,
    Download,
    FileCog,
    FolderOpen,
    GripVertical,
    Monitor,
    Moon,
    Play,
    Plus,
    ChevronDown,
    RefreshCw,
    Settings,
    Square,
    Sun,
    TerminalSquare,
    Trash2,
  } from "lucide-svelte";

  type CapturerState =
    | "starting"
    | "running"
    | "stopping"
    | "exited"
    | "crashed";

  interface CapturerInstance {
    id: number;
    name: string;
    state: CapturerState;
    pid: number | null;
    bindAddr: string | null;
    profileId: string | null;
    uptimeSecs: number;
    lastStderr: string | null;
    stderrTail: string[];
    exitCode: number | null;
  }

  interface CapturerRuntimeStatus {
    id: number;
    state: CapturerState;
    pid: number | null;
    bindAddr: string | null;
    healthy: boolean;
    uptimeSecs: number;
    note: string | null;
    snapshot: Record<string, unknown> | null;
    fps?: CapturerOutputFps | null;
  }

  interface PartDiagnostic {
    group: string;
    status: "estimated" | "held" | "recovering" | "lost" | "off" | string;
    confidence: number;
  }

  interface CapturerOutputFps {
    intervalSecs: number;
    vmcDatagramsPerSec: number;
    vmcPacketsPerSec: number;
    vrcOscDatagramsPerSec: number;
    vrcOscPacketsPerSec: number;
    zenohFramesPerSec: number;
    sources: CapturerSourceFps[];
  }

  interface CapturerSourceFps {
    kind: string;
    streamId: string;
    sourceId: string;
    rawPerSec: number;
    framesPerSec: number;
    observedSourceFps?: number | null;
  }

  interface ProfileSummary {
    id: string;
    name: string;
    note: string;
    iconPath?: string | null;
    group?: string;
    engine?: string | null;
    runtimeSelection?: {
      fps?: number | null;
      engine?: string | null;
      device?: string | null;
      resolution?: string | null;
      vmcReceiveListenAddr?: string | null;
      ifacialmocapReceiveListenAddr?: string | null;
      modifier?: {
        headEnabled?: boolean | null;
        faceEnabled?: boolean | null;
        facePoseModel?: {
          enabled?: boolean | null;
          neutralNoseDropEyeMouth?: number | null;
        } | null;
      } | null;
    } | null;
    pipelineComponents?: {
      input?: string | null;
      inputFps?: number | null;
      inputWidth?: number | null;
      inputHeight?: number | null;
      inputPixelFormat?: string | null;
      inputDenoiseMode?: string | null;
      inputDenoiseTemporalIirHz?: number | null;
    } | null;
  }

  interface ProfileRuntimeView {
    fps: number | null;
    vmcEnabled: boolean | null;
    vmcTargetAddr: string | null;
    vrcOscEnabled: boolean | null;
    vrcOscTargetAddr: string | null;
    vrcOscSendOnlyWhenVrchatRunning: boolean | null;
    vrcOscProcessPollIntervalSecs: number | null;
    vrcOscParameterPrefix: string | null;
    zenohEnabled: boolean | null;
    zenohKeyExpr: string | null;
    zenohTopicMode: string | null;
    zenohStreamId: string | null;
    zenohProducer: string | null;
    engine: string | null;
    device: string | null;
    resolution: string | null;
    mediaPipeRunningMode: string | null;
    mediaPipeHolisticEnabled: boolean | null;
    mediaPipeDelegate: string | null;
    mediaPipeNumThreads: number | null;
    mediaPipeHolisticFlowLimiterEnabled: boolean | null;
    mediaPipeHolisticFlowLimiterMaxInFlight: number | null;
    mediaPipeHolisticFlowLimiterMaxInQueue: number | null;
    vmcReceiveListenAddr: string | null;
    ifacialmocapReceiveListenAddr: string | null;
    modifierHeadEnabled: boolean | null;
    modifierFaceEnabled: boolean | null;
    modifierHandsEnabled: boolean | null;
    modifierArmsIkEnabled: boolean | null;
    modifierTorsoEnabled: boolean | null;
    modifierLegsEnabled: boolean | null;
    modifierFeetEnabled: boolean | null;
    modifierTorsoPitchScale: number | null;
    modifierNeutralCalibrationEnabled: boolean | null;
    modifierNeutralCalibrationSampleCount: number | null;
    modifierNeutralCalibrationPose: string | null;
    modifierMirrorMode: string | null;
    modifierEyeOpenBias: number | null;
    modifierSmoothingPreset: string | null;
    modifierSmoothingEmaEnabled: boolean | null;
    modifierSmoothingEmaAlpha: number | null;
    modifierSmoothingOneEuroEnabled: boolean | null;
    modifierSmoothingConfidenceAdaptiveCutoff: boolean | null;
    modifierAdaptiveMinCutoffHz: number | null;
    modifierAdaptiveBeta: number | null;
    modifierAdaptiveDerivativeCutoffHz: number | null;
    modifierFacePoseModelEnabled: boolean | null;
    modifierFacePoseModelNeutralNoseDropEyeMouth: number | null;
    modifierFacePoseModelSampleCount: number | null;
    modifierAnatomicalConstraints: boolean | null;
    modifierHoldLostLandmarks: boolean | null;
    modifierEaseRecovery: boolean | null;
    modifierLimitRotationJumps: boolean | null;
    modifierHeadSourceSwitchBlend: boolean | null;
    modifierHeadFromFaceMatrix: boolean | null;
    modifierLostSignalBehavior: string | null;
    modifierLostSignalRestPoseBlend: number | null;
    modifierLostSignalHoldSeconds: number | null;
    modifierLostSignalHeadBehavior: string | null;
    modifierLostSignalHeadRestPoseBlend: number | null;
    modifierLostSignalHeadHoldSeconds: number | null;
    modifierLostSignalHandsBehavior: string | null;
    modifierLostSignalHandsRestPoseBlend: number | null;
    modifierLostSignalHandsHoldSeconds: number | null;
    modifierLostSignalArmsBehavior: string | null;
    modifierLostSignalArmsRestPoseBlend: number | null;
    modifierLostSignalArmsHoldSeconds: number | null;
    modifierLostSignalRecoverySeconds: number | null;
  }

  interface ProfilePipelineView {
    engine: string | null;
    input: string | null;
    postProcess: string | null;
    inputPath: string | null;
    inputFps: number | null;
    inputWidth: number | null;
    inputHeight: number | null;
    inputPixelFormat: string | null;
    inputRepeat: boolean | null;
    inputFfmpegPath: string | null;
    inputDenoiseMode: string | null;
    inputDenoiseTemporalIirHz: number | null;
    inputResizeEnabled: boolean | null;
    inputResizeAxis: string | null;
    inputResizeReference: number | null;
    inputResizeWidth: number | null;
    inputResizeHeight: number | null;
    inputResizePreserveAspect: boolean | null;
    inputResizePadColor: string | null;
  }

  /// Engine Type. ``runtime_selection.engine`` を source of truth とする。
  type EngineType = "mediapipe-native" | "vmc" | "ifacialmocap";

  /// MediaPipe Engine 系専用の Source Type。``pipeline_components.input`` に保存する。
  ///
  /// 旧 `"webcam-nokhwa"` は GUI 上で `"Webcam (MediaFoundation)"` と表示するよう
  /// に rename。profile に `"webcam-nokhwa"` が残っていても `resolveMediaPipeSourceType`
  /// で `"webcam-mediafoundation"` に正規化する (TOML 上は今後 `"webcam-mediafoundation"`
  /// を新規書き込み値とする)。
  type MediaPipeSourceType =
    | "webcam-directshow"
    | "webcam-mediafoundation"
    | "file-image"
    | "file-video";

  /// Tauri backend の `enumerate_webcams` 戻り値。
  type WebcamDeviceDto = { id: string; label: string };
  /// Tauri backend の `enumerate_webcam_formats` 戻り値。
  type WebcamFormatDto = {
    width: number;
    height: number;
    fps: number | null;
    pixelFormat: string;
    label: string;
  };

  /// よくある解像度プリセット (Webcam / Image / Video 共用)。テキスト入力をさせず、
  /// 選択肢ベースの GUI を提供するためのリスト。``custom`` は profile TOML で直接設定する想定。
  const resolutionPresets = [
    { value: "", label: "(default 640x480)" },
    { value: "640x360", label: "640x360 (nHD 16:9)" },
    { value: "640x480", label: "640x480 (VGA 4:3)" },
    { value: "960x540", label: "960x540 (qHD 16:9)" },
    { value: "1280x720", label: "1280x720 (HD 16:9)" },
    { value: "1920x1080", label: "1920x1080 (FHD 16:9)" },
  ];

  const fpsPresets = [
    { value: null, label: "(follow input)" },
    { value: 24, label: "24" },
    { value: 30, label: "30" },
    { value: 60, label: "60" },
    { value: 90, label: "90" },
    { value: 120, label: "120" },
    { value: 144, label: "144" },
    { value: 240, label: "240" },
  ];

  const mediaPipeThreadPresets = [
    { value: null, label: "(default: 2)" },
    { value: 1, label: "1" },
    { value: 2, label: "2" },
    { value: 4, label: "4" },
    { value: 8, label: "8" },
    { value: 12, label: "12" },
    { value: 16, label: "16" },
  ];

  const mediaPipeFlowLimiterPresets = [
    { value: 1, label: "1" },
    { value: 2, label: "2" },
    { value: 3, label: "3" },
    { value: 4, label: "4" },
  ];

  interface ProfileDetail {
    id: string;
    name: string;
    note: string;
    path: string;
    iconPath: string | null;
    group: string;
    runtime: ProfileRuntimeView;
    pipeline: ProfilePipelineView;
  }

  type ThemeMode = "light" | "dark" | "system";
  type TabId = "capturers" | "profiles" | "logs" | "app";

  /// Tauri backend (Rust) の `AppRuntimeSettings` (= `%APPDATA%/UN Motion/settings.toml`)
  /// と双方向に同期する設定。UN Avatar Supervisor の `AppSettings` と
  /// フィールド名 / 既定値を 1:1 で揃え、`sync_app_settings` で 1 リクエストで保存する。
  type AppSettings = {
    stopCapturersOnExit: boolean;
    systemTrayEnabled: boolean;
    minimizeToTray: boolean;
    closeToTrayWhileRunning: boolean;
    startMinimizedToTray: boolean;
    /// Profiles → Quick Launch を押したときに Capturers タブへ自動遷移するか。
    /// UN Avatar 側でも同じ Quick Launch 用語へ揃える前提の設定。
    jumpToCapturersOnQuickLaunch: boolean;
    /// Capturers の launch target として選択中の profile/group を起動時に自動起動するか。
    autoLaunchSelectedOnStartup: boolean;
    themeMode: ThemeMode;
    consoleWindowX: number | null;
    consoleWindowY: number | null;
    consoleWindowWidth: number | null;
    consoleWindowHeight: number | null;
    apiWorkerThreads: number;
    /// UI 表示言語 (BCP-47, 例: "ja-JP" / "en-US")。空文字なら起動時に OS locale から自動解決。
    /// 値はサーバー側で正規化され、ロード時にもサーバーが filled-in したものが返ってくる。
    locale: string;
    externalToolsFfmpegPath: string | null;
    calibrationStartDelaySeconds: number;
    calibrationSampleCount: number;
    calibrationSoundVolume: number;
    calibrationCountdownSoundPath: string | null;
    calibrationStartSoundPath: string | null;
    snapshotSaveDir: string | null;
    snapshotSaveAnalysisExtras: boolean;
  };

  type VideoFileMetadata = {
    width: number | null;
    height: number | null;
    fps: number | null;
    fpsRounded: number | null;
    source: string;
  };

  const defaultAppSettings: AppSettings = {
    stopCapturersOnExit: true,
    systemTrayEnabled: false,
    minimizeToTray: true,
    closeToTrayWhileRunning: true,
    startMinimizedToTray: false,
    jumpToCapturersOnQuickLaunch: false,
    autoLaunchSelectedOnStartup: false,
    themeMode: "system",
    consoleWindowX: null,
    consoleWindowY: null,
    consoleWindowWidth: null,
    consoleWindowHeight: null,
    apiWorkerThreads: 2,
    locale: "",
    externalToolsFfmpegPath: null,
    calibrationStartDelaySeconds: 3,
    calibrationSampleCount: 45,
    calibrationSoundVolume: 0.75,
    calibrationCountdownSoundPath: null,
    calibrationStartSoundPath: null,
    snapshotSaveDir: null,
    snapshotSaveAnalysisExtras: false,
  };

  const defaultIconSrc = "/un-motion-artwork-supervisor.png";
  const defaultCalibrationCountdownSound =
    "/sounds/un-calibration-sound-pun.flac";
  const defaultCalibrationStartSound = "/sounds/un-calibration-sound-pon.flac";
  const poseImageI = "/poses/usagi-pose-I.png";
  const poseImageU = "/poses/usagi-pose-U.png";
  const poseImageT = "/poses/usagi-pose-T-wrist-front.png";
  const launchTargetStorageKey = "un-motion-supervisor.launch-target-id";
  const calibrationPoseOptions: {
    kind: CalibrationPoseKind;
    shortLabel: string;
    label: string;
    imageSrc: string;
  }[] = [
    {
      kind: "U",
      shortLabel: "U",
      label: "Uポーズ",
      imageSrc: poseImageU,
    },
    {
      kind: "T",
      shortLabel: "Twf",
      label: "T-wrist-frontポーズ",
      imageSrc: poseImageT,
    },
    {
      kind: "I",
      shortLabel: "I",
      label: "Iポーズ",
      imageSrc: poseImageI,
    },
  ];

  let appSettings = $state<AppSettings>({ ...defaultAppSettings });
  let appSettingsReady = $state(false);
  let activeTab: TabId = $state("capturers");
  let appVersion = $state("");
  // Language 一覧は起動後にバックエンドの `i18n_available_locales` で置き換わる（初期表示用の仮値）。
  let availableLocales = $state<string[]>(["ja-JP", "en-US"]);
  const logicalCoreCount =
    typeof navigator === "undefined"
      ? 2
      : Math.max(1, Math.floor(navigator.hardwareConcurrency || 2));

  function hasTauriRuntime(): boolean {
    if (typeof window === "undefined") return false;
    const w = window as any;
    return Boolean(w.__TAURI__ || w.__TAURI_INTERNALS__);
  }

  function loadLaunchTargetId(): string {
    if (typeof window === "undefined") return "";
    return window.localStorage.getItem(launchTargetStorageKey) ?? "";
  }

  function saveLaunchTargetId(value: string): void {
    if (typeof window === "undefined") return;
    if (value.trim()) {
      window.localStorage.setItem(launchTargetStorageKey, value);
    } else {
      window.localStorage.removeItem(launchTargetStorageKey);
    }
  }

  function isValidLaunchTarget(
    value: string,
    items: ProfileSummary[],
  ): boolean {
    if (!value) return false;
    if (value.startsWith("group:")) {
      const group = value.slice("group:".length).trim();
      return (
        group.length > 0 && items.some((p) => (p.group ?? "").trim() === group)
      );
    }
    return items.some((p) => p.id === value);
  }

  function pickInitialLaunchTargetId(
    current: string,
    selected: string | null,
    items: ProfileSummary[],
  ): string {
    if (isValidLaunchTarget(current, items)) return current;
    if (selected && isValidLaunchTarget(selected, items)) return selected;
    return items[0]?.id ?? "";
  }

  function iconSrc(path: string | null | undefined): string {
    if (!path) return defaultIconSrc;
    if (
      path.startsWith("http://") ||
      path.startsWith("https://") ||
      path.startsWith("/")
    ) {
      return path;
    }
    if (
      path.endsWith("un-motion-design-master.png") ||
      path.endsWith("un-motion-artwork-supervisor.png")
    ) {
      return defaultIconSrc;
    }
    if (hasTauriRuntime()) return convertFileSrc(path);
    return defaultIconSrc;
  }

  function runningCountForProfile(profileId: string): number {
    return capturers.filter(
      (capturer) =>
        capturer.profileId === profileId &&
        (capturer.state === "running" || capturer.state === "starting"),
    ).length;
  }

  async function browseProfileIcon(): Promise<void> {
    if (!selectedProfileId) return;
    try {
      const path = await invoke<string | null>("pick_file_path", {
        kind: "icon",
      });
      if (path != null) {
        await updateProfileField("icon_path", path);
      }
    } catch (error) {
      errorMessage = `pick_file_path failed: ${String(error)}`;
    }
  }

  async function browseFfmpegExecutable(): Promise<void> {
    try {
      const path = await invoke<string | null>("pick_file_path", {
        kind: "ffmpeg",
      });
      if (path != null) {
        setAppSetting("externalToolsFfmpegPath", path);
      }
    } catch (error) {
      errorMessage = `pick ffmpeg failed: ${String(error)}`;
    }
  }

  async function browseCalibrationSound(
    kind: "countdown" | "start",
  ): Promise<void> {
    try {
      const path = await invoke<string | null>("pick_file_path", {
        kind: "sound",
      });
      if (path) {
        setAppSetting(
          kind === "countdown"
            ? "calibrationCountdownSoundPath"
            : "calibrationStartSoundPath",
          path,
        );
      }
    } catch (error) {
      errorMessage = `pick sound file failed: ${String(error)}`;
    }
  }

  function detectSystemTheme(): "light" | "dark" {
    if (
      typeof window === "undefined" ||
      typeof window.matchMedia !== "function"
    ) {
      return "dark";
    }
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }

  let systemThemePreference = $state<"light" | "dark">(detectSystemTheme());
  const effectiveTheme = $derived<"light" | "dark">(
    appSettings.themeMode === "system"
      ? systemThemePreference
      : (appSettings.themeMode as "light" | "dark"),
  );

  async function loadBackendSettings() {
    try {
      const result = await invoke<AppSettings>("get_app_settings");
      if (result) {
        appSettings = { ...defaultAppSettings, ...result };
        // backend が解決した locale (永続化済 or 起動時 OS 自動解決済) を svelte-i18n へ反映する。
        // 空文字なら resolve_default_locale を再取得して適用 (二重解決にはなるが起動経路を一本化)。
        const effective =
          appSettings.locale ||
          (await invoke<string>("i18n_resolve_default_locale").catch(
            () => "ja-JP",
          ));
        setUiLocale(effective);
      }
    } catch (error) {
      console.error("get_app_settings failed:", error);
    } finally {
      appSettingsReady = true;
    }
  }

  async function syncAppSettingsToBackend(
    settings: AppSettings,
  ): Promise<void> {
    if (!appSettingsReady) return;
    try {
      const result = await invoke<AppSettings>("sync_app_settings", {
        settings,
      });
      if (result) {
        appSettings = { ...defaultAppSettings, ...result };
      }
    } catch (error) {
      if (!hasTauriRuntime()) return;
      errorMessage = `sync_app_settings failed: ${String(error)}`;
    }
  }

  function setAppSetting<K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K],
  ): void {
    appSettings = { ...appSettings, [key]: value };
    void syncAppSettingsToBackend(appSettings);
  }

  function setThemeMode(mode: ThemeMode): void {
    setAppSetting("themeMode", mode);
  }

  function soundSrc(path: string | null | undefined, fallback: string): string {
    if (!path) return fallback;
    if (
      path.startsWith("http://") ||
      path.startsWith("https://") ||
      path.startsWith("/")
    ) {
      return path;
    }
    if (hasTauriRuntime()) return convertFileSrc(path);
    return fallback;
  }

  async function playCalibrationSound(
    path: string | null | undefined,
    fallback: string,
  ): Promise<void> {
    if (typeof Audio === "undefined") return;
    const audio = new Audio(soundSrc(path, fallback));
    audio.volume = clampNumber(
      appSettings.calibrationSoundVolume ?? 0.75,
      0,
      1,
    );
    try {
      await audio.play();
    } catch {
      // webview に音声再生をブロックされても calibration 自体は続行する。
    }
  }

  function sleepMs(ms: number): Promise<void> {
    return new Promise((resolve) => window.setTimeout(resolve, ms));
  }

  async function openExternalLink(url: string): Promise<void> {
    try {
      await invoke("open_external_url", { url });
      return;
    } catch (error) {
      if (hasTauriRuntime()) {
        errorMessage = `open_external_url failed: ${String(error)}`;
      }
    }
    try {
      window.open(url, "_blank", "noopener");
    } catch {
      // 失敗時も URL は画面に出ているので致命扱いしない。
    }
  }

  async function stopAllCapturers() {
    busy = true;
    errorMessage = "";
    try {
      await invoke("stop_all_capturers");
      message = "All capturers stop requested";
      await refreshCapturers();
    } catch (error) {
      errorMessage = `stop_all_capturers failed: ${error}`;
    } finally {
      busy = false;
    }
  }

  let capturers: CapturerInstance[] = $state([]);
  let runtimeStatuses: Record<number, CapturerRuntimeStatus> = $state({});
  let selectedCapturerId: number | null = $state(null);
  let showStoppedCapturers = $state(false);

  let profiles: ProfileSummary[] = $state([]);
  let launchTargetId = $state(loadLaunchTargetId());
  let selectedProfileId: string | null = $state(null);
  let profilesReady = $state(false);
  let persistedSelectedProfileId: string | null = $state(null);
  let draggedProfileId: string | null = $state(null);
  let profileHint = $state("");
  let settingsHint = $state("");
  let profilePointerDrag: {
    id: string;
    startX: number;
    startY: number;
    currentX: number;
    currentY: number;
    offsetX: number;
    offsetY: number;
    width: number;
    height: number;
    active: boolean;
  } | null = $state(null);
  let suppressProfileClick = $state(false);
  let profileDetail: ProfileDetail | null = $state(null);
  let selectedCapturerProfileDetail: ProfileDetail | null = $state(null);

  let capturersProfileMenuOpen = $state(false);
  const profileGroups = $derived(
    Array.from(
      new Set(
        profiles
          .map((p) => (p.group ?? "").trim())
          .filter((group) => group.length > 0),
      ),
    ).sort((a, b) => a.localeCompare(b)),
  );
  const launchProfileId = $derived(
    launchTargetId.startsWith("group:") ? "" : launchTargetId,
  );
  const launchProfileSummary = $derived(
    profiles.find((p) => p.id === launchProfileId) ?? null,
  );
  const launchGroupName = $derived(
    launchTargetId.startsWith("group:")
      ? launchTargetId.slice("group:".length)
      : "",
  );
  const launchGroupProfiles = $derived(
    launchGroupName
      ? profiles.filter((p) => (p.group ?? "").trim() === launchGroupName)
      : [],
  );
  const defaultProfileHint = $derived($_("profiles.hints.default"));
  const defaultSettingsHint = $derived($_("settings.hints.default"));

  let busy = $state(false);
  let message = $state("");
  let errorMessage = $state("");
  let startupAutoLaunchAttempted = false;

  let pollHandle: ReturnType<typeof setInterval> | null = null;

  function updateProfileHintFromEvent(event: Event): void {
    const target = event.target instanceof HTMLElement ? event.target : null;
    const hint = target?.closest<HTMLElement>("[data-hint]")?.dataset.hint;
    if (hint) profileHint = hint;
  }

  function clearProfileHint(): void {
    profileHint = "";
  }

  function updateSettingsHintFromEvent(event: Event): void {
    const target = event.target instanceof HTMLElement ? event.target : null;
    const hint = target?.closest<HTMLElement>("[data-hint]")?.dataset.hint;
    if (hint) settingsHint = hint;
  }

  function clearSettingsHint(): void {
    settingsHint = "";
  }

  // ---------------------------------------------------------------------------
  // Logs タブの表示状態。
  // ---------------------------------------------------------------------------
  // UN Avatar Supervisor の Logs タブ (1 つの `<pre>` に全 renderer の stderr を
  // 連結) を踏襲しつつ、UN Motion 固有の要望に応じて以下を追加:
  // * `logsCapturerFilter`: Capturer id ("all" or 数値) で絞り込み
  // * `logsTextFilter`: 行内部分一致での絞り込み (空文字なら全行)
  // * `logsAutoscroll`: 新ライン到着時に `<pre>` を末尾へスクロール
  // * `Copy all` ボタンで現在表示中の lines を一括クリップボードコピー
  //   (実機 debug 中に Cursor へ貼り付ける作業が頻発し、毎回 textarea 全選択する
  //    のはユーザー負荷が大きい、というフィードバック)
  let logsCapturerFilter = $state<"all" | number>("all");
  let logsTextFilter = $state("");
  let logsAutoscroll = $state(true);
  let logsViewRef: HTMLElement | null = $state(null);
  // Copy ボタン押下時の一時的なフィードバック表示 ("Copied" → 1.5s で戻る)
  let logsCopyFlash = $state(false);
  let logsCopyFlashTimer: ReturnType<typeof setTimeout> | null = null;

  /// 現在のフィルタ設定に従って Capturer 群の stderrTail を 1 つの string[] に畳む。
  /// 各行に `[<id> <name>]` プレフィックスを付けてどの Capturer の行か識別可能にする。
  function filteredLogLines(): string[] {
    const query = logsTextFilter.trim().toLowerCase();
    const includeAll = logsCapturerFilter === "all";
    const lines: string[] = [];
    for (const cap of capturers) {
      if (!includeAll && cap.id !== logsCapturerFilter) continue;
      if (cap.stderrTail.length === 0) {
        const placeholder = `[${cap.id} ${cap.name}] (${cap.state}; no stderr yet)`;
        if (!query || placeholder.toLowerCase().includes(query)) {
          lines.push(placeholder);
        }
        continue;
      }
      for (const raw of cap.stderrTail) {
        const prefixed = `[${cap.id} ${cap.name}] ${raw}`;
        if (!query || prefixed.toLowerCase().includes(query)) {
          lines.push(prefixed);
        }
      }
    }
    return lines;
  }

  /// Per-capturer view 用: 1 つの Capturer の stderrTail からフィルタ通過後の
  /// 行を返す (プレフィックス無し)。
  function filteredLinesForCapturer(capturerId: number): string[] {
    const query = logsTextFilter.trim().toLowerCase();
    const cap = capturers.find((c) => c.id === capturerId);
    if (!cap) return [];
    if (cap.stderrTail.length === 0) return [];
    if (!query) return cap.stderrTail;
    return cap.stderrTail.filter((line) => line.toLowerCase().includes(query));
  }

  function jumpToCapturerLog(cap: CapturerInstance): void {
    selectedCapturerId = cap.id;
    logsCapturerFilter = cap.id;
    logsTextFilter = "";
    logsLayout = "unified";
    activeTab = "logs";
    queueMicrotask(scrollLogsToBottom);
  }

  /// 1 行の severity を粗く判定する。tracing 既定 (ANSI 無効化済み) と
  /// MediaPipe Native の eprintln 系から漏れる行が混ざる前提で、雑に prefix で見る。
  function lineSeverity(
    line: string,
  ): "error" | "warn" | "info" | "debug" | "" {
    if (/\b(ERROR|FATAL)\b/.test(line)) return "error";
    if (/\b(WARN|WARNING)\b/.test(line)) return "warn";
    if (/\b(INFO)\b/.test(line)) return "info";
    if (/\b(DEBUG|TRACE)\b/.test(line)) return "debug";
    return "";
  }

  /// "Per-capturer" / "Unified" toggle。既定は per-capturer (プロセスごとに
  /// 分かれて見えるので「どの Capturer が何を吐いたか」が直観的に分かる)。
  type LogsLayout = "per-capturer" | "unified";
  let logsLayout = $state<LogsLayout>("per-capturer");

  /// Capturer card ごとの展開状態。デフォルトは Running のときだけ開く。
  let logsExpanded = $state<Record<number, boolean>>({});
  function isCapturerLogExpanded(cap: CapturerInstance): boolean {
    const explicit = logsExpanded[cap.id];
    if (explicit !== undefined) return explicit;
    return (
      cap.state === "running" ||
      cap.state === "starting" ||
      ((cap.state === "exited" || cap.state === "crashed") &&
        cap.stderrTail.length > 0)
    );
  }
  function toggleCapturerLogExpanded(cap: CapturerInstance): void {
    logsExpanded[cap.id] = !isCapturerLogExpanded(cap);
  }

  async function saveLogsToFile(): Promise<void> {
    const text = filteredLogLines().join("\n");
    if (hasTauriRuntime()) {
      try {
        const path = await invoke<string>("save_supervisor_logs", {
          content: text,
          filePrefix: "un-motion-supervisor",
        });
        errorMessage = `Saved ${path.split(/[/\\]/).pop() ?? path}`;
      } catch (error) {
        errorMessage = String(error);
      }
      return;
    }
    const blob = new Blob([text], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const ts = new Date()
      .toISOString()
      .replace(/[:.]/g, "-")
      .replace("T", "_")
      .slice(0, 19);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `un-motion-supervisor-logs-${ts}.txt`;
    document.body.appendChild(anchor);
    anchor.click();
    document.body.removeChild(anchor);
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  async function revealSupervisorLogsDir(): Promise<void> {
    if (!hasTauriRuntime()) {
      errorMessage = "Browser preview: open folder requires Tauri";
      return;
    }
    try {
      await invoke("reveal_supervisor_logs_dir");
    } catch (error) {
      errorMessage = String(error);
    }
  }

  /// `<pre>` を末尾へスクロールする。autoscroll が ON のときだけ呼ぶ。
  function scrollLogsToBottom(): void {
    if (!logsViewRef) return;
    logsViewRef.scrollTop = logsViewRef.scrollHeight;
  }

  async function copyLogsToClipboard(): Promise<void> {
    const text = filteredLogLines().join("\n");
    try {
      await navigator.clipboard.writeText(text);
      logsCopyFlash = true;
      if (logsCopyFlashTimer) clearTimeout(logsCopyFlashTimer);
      logsCopyFlashTimer = setTimeout(() => {
        logsCopyFlash = false;
      }, 1500);
    } catch (error) {
      errorMessage = `clipboard write failed: ${String(error)}`;
    }
  }

  $effect(() => {
    document.documentElement.dataset.theme = effectiveTheme;
  });

  $effect(() => {
    if (
      typeof window === "undefined" ||
      typeof window.matchMedia !== "function"
    ) {
      return;
    }
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      systemThemePreference = mq.matches ? "dark" : "light";
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  });

  $effect(() => {
    if (activeTab !== "capturers") {
      capturersProfileMenuOpen = false;
    }
  });

  $effect(() => {
    saveLaunchTargetId(launchTargetId);
  });

  $effect(() => {
    const profileId = selectedProfileId;
    if (!profilesReady) return;
    if (profileId === persistedSelectedProfileId) return;
    void persistSelectedProfileId(profileId);
  });

  // Logs タブが選択中、かつ autoscroll が ON、かつ Capturer の stderrTail に
  // 変化があった場合に末尾へスクロール。`capturers` 全体を依存とすることで
  // 各 Capturer の stderrTail.push をトリガにできる ($state 配列の中身を Svelte
  // が深く追跡しない代わり、`refreshCapturers` で配列全体が置換されるため
  // identity 変化で発火)。
  $effect(() => {
    if (activeTab !== "logs" || !logsAutoscroll) return;
    // capturers と filter state を依存に含めるため明示的に参照する。
    void capturers;
    void logsTextFilter;
    void logsCapturerFilter;
    // DOM の更新を 1 tick 待ってからスクロール。
    queueMicrotask(scrollLogsToBottom);
  });

  $effect(() => {
    const tableCapturers = showStoppedCapturers ? capturers : liveCapturers();
    const selectedExists = tableCapturers.some((c) => c.id === selectedCapturerId);
    if (!selectedExists && tableCapturers.length > 0) {
      selectedCapturerId = tableCapturers[0].id;
    } else if (tableCapturers.length === 0) {
      selectedCapturerId = null;
    }
  });

  const runningCount = $derived(
    capturers.filter((capturer) => capturer.state === "running").length,
  );
  const issueCount = $derived(
    capturers.filter((capturer) => capturer.state === "crashed").length,
  );
  const capturerTableCapturers = $derived(
    showStoppedCapturers ? capturers : liveCapturers(),
  );

  const selectedCapturer = $derived(
    capturerTableCapturers.find((capturer) => capturer.id === selectedCapturerId) ?? null,
  );
  const selectedRuntime = $derived(
    selectedCapturerId == null
      ? null
      : (runtimeStatuses[selectedCapturerId] ?? null),
  );
  const selectedCapturerProfileId = $derived(
    selectedCapturer?.profileId ?? null,
  );
  const selectedCapturerProfileSummary = $derived(
    selectedCapturerProfileId == null
      ? null
      : (profiles.find((profile) => profile.id === selectedCapturerProfileId) ??
          null),
  );
  $effect(() => {
    const profileId = selectedCapturerProfileId;
    if (!profileId) {
      selectedCapturerProfileDetail = null;
      return;
    }
    void refreshSelectedCapturerProfileDetail(profileId);
  });

  const engineType = $derived<EngineType>(resolveEngineType(profileDetail));
  const mediaPipeSourceType = $derived<MediaPipeSourceType>(
    resolveMediaPipeSourceType(profileDetail),
  );
  const isMediaPipeEngine = $derived(engineType === "mediapipe-native");

  onMount(async () => {
    appVersion = await invoke<string>("app_version").catch(() => "?");
    availableLocales = await invoke<string[]>("i18n_available_locales").catch(
      () => ["ja-JP", "en-US"],
    );
    await Promise.all([
      loadBackendSettings(),
      refreshCapturers(),
      refreshProfiles(),
    ]);
    await maybeAutoLaunchSelectedOnStartup();
    pollHandle = setInterval(() => {
      void refreshCapturers();
      void refreshRuntimeStatuses();
    }, 1500);
  });

  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
    cancelProfilePointerDrag();
  });

  async function refreshCapturers() {
    try {
      capturers = await invoke<CapturerInstance[]>("list_capturers");
    } catch (error) {
      errorMessage = `list_capturers failed: ${String(error)}`;
    }
  }

  async function refreshProfiles() {
    try {
      profiles = await invoke<ProfileSummary[]>("list_profiles");
      const storedSelectedProfileId = profilesReady
        ? selectedProfileId
        : await invoke<string>("selected_profile_id").catch(() => "");
      const stillExists =
        storedSelectedProfileId !== null &&
        profiles.some((profile) => profile.id === storedSelectedProfileId);
      if (!stillExists) {
        selectedProfileId = profiles[0]?.id ?? null;
      } else {
        selectedProfileId = storedSelectedProfileId;
      }
      if (!profilesReady) {
        persistedSelectedProfileId = selectedProfileId;
      }
      launchTargetId = pickInitialLaunchTargetId(
        launchTargetId,
        selectedProfileId,
        profiles,
      );
      if (selectedProfileId) {
        await refreshProfileDetail(selectedProfileId);
      } else {
        profileDetail = null;
      }
      profilesReady = true;
    } catch (error) {
      errorMessage = `list_profiles failed: ${String(error)}`;
    }
  }

  async function persistSelectedProfileId(profileId: string | null) {
    try {
      await invoke("set_selected_profile_id", { profileId });
      persistedSelectedProfileId = profileId;
    } catch (error) {
      console.warn("set_selected_profile_id failed", error);
    }
  }

  async function refreshProfileDetail(profileId: string) {
    try {
      profileDetail = await invoke<ProfileDetail>("get_profile_detail", {
        profileId,
      });
    } catch (error) {
      errorMessage = `get_profile_detail failed: ${String(error)}`;
      profileDetail = null;
    }
  }

  async function openFfmpegHome(): Promise<void> {
    try {
      await invoke("open_ffmpeg_home");
      return;
    } catch (error) {
      if (hasTauriRuntime()) {
        errorMessage = `open_ffmpeg_home failed: ${String(error)}`;
        return;
      }
    }
    try {
      window.open("https://ffmpeg.org/", "_blank", "noopener");
    } catch {
      // browser preview 用 fallback のみ。
    }
  }

  async function revealProfilesDir(): Promise<void> {
    if (!hasTauriRuntime()) {
      errorMessage = "Browser preview: open folder requires Tauri";
      return;
    }
    try {
      await invoke("reveal_profiles_dir");
      message = "Opened profiles folder";
    } catch (error) {
      errorMessage = String(error);
    }
  }

  function previewProfileReorder(sourceId: string, insertIndex: number): void {
    const sourceIndex = profiles.findIndex(
      (profile) => profile.id === sourceId,
    );
    if (sourceIndex < 0) return;
    insertIndex = Math.max(0, Math.min(insertIndex, profiles.length));
    if (sourceIndex < insertIndex) insertIndex -= 1;
    if (sourceIndex === insertIndex) return;
    const next = [...profiles];
    const [moved] = next.splice(sourceIndex, 1);
    next.splice(insertIndex, 0, moved);
    profiles = next;
  }

  async function saveProfilesOrder(): Promise<void> {
    try {
      await invoke<ProfileSummary[]>("reorder_profiles", {
        profileIds: profiles.map((profile) => profile.id),
      });
      message = "Reordered profiles";
    } catch (error) {
      errorMessage = String(error);
      await refreshProfiles();
    }
  }

  function beginProfilePointerDrag(
    event: PointerEvent,
    profileId: string,
  ): void {
    if (busy || event.button !== 0) return;
    const card = (event.currentTarget as HTMLElement).closest<HTMLElement>(
      "[data-profile-id]",
    );
    const rect = card?.getBoundingClientRect();
    if (!rect) return;
    profilePointerDrag = {
      id: profileId,
      startX: event.clientX,
      startY: event.clientY,
      currentX: event.clientX,
      currentY: event.clientY,
      offsetX: event.clientX - rect.left,
      offsetY: event.clientY - rect.top,
      width: rect.width,
      height: rect.height,
      active: false,
    };
    window.addEventListener("pointermove", updateProfilePointerDrag);
    window.addEventListener("pointerup", finishProfilePointerDrag);
    window.addEventListener("pointercancel", cancelProfilePointerDrag);
    document.documentElement.classList.add("profile-dragging");
    event.preventDefault();
    event.stopPropagation();
  }

  function removeProfileDragListeners(): void {
    window.removeEventListener("pointermove", updateProfilePointerDrag);
    window.removeEventListener("pointerup", finishProfilePointerDrag);
    window.removeEventListener("pointercancel", cancelProfilePointerDrag);
    document.documentElement.classList.remove("profile-dragging");
  }

  function cancelProfilePointerDrag(): void {
    removeProfileDragListeners();
    profilePointerDrag = null;
    draggedProfileId = null;
  }

  function updateProfilePointerDrag(event: PointerEvent): void {
    const drag = profilePointerDrag;
    if (!drag) return;
    const distance =
      Math.abs(event.clientX - drag.startX) +
      Math.abs(event.clientY - drag.startY);
    const active = drag.active || distance > 5;
    profilePointerDrag = {
      ...drag,
      currentX: event.clientX,
      currentY: event.clientY,
      active,
    };
    if (active && !drag.active) {
      draggedProfileId = drag.id;
      suppressProfileClick = true;
    }
    if (!active) return;
    event.preventDefault();
    event.stopPropagation();
    scrollProfileListDuringDrag(event.clientY);
    const cards = Array.from(
      document.querySelectorAll<HTMLElement>("[data-profile-id]"),
    );
    const insertIndex = cards.findIndex((card) => {
      if (card.dataset.profileId === drag.id) return false;
      const rect = card.getBoundingClientRect();
      return event.clientY < rect.top + rect.height / 2;
    });
    previewProfileReorder(
      drag.id,
      insertIndex < 0 ? profiles.length : insertIndex,
    );
  }

  function scrollProfileListDuringDrag(clientY: number): void {
    const list = document.querySelector<HTMLElement>(".setting-list");
    if (!list) return;
    const rect = list.getBoundingClientRect();
    const edge = 56;
    if (clientY < rect.top + edge) {
      list.scrollBy({ top: -10, behavior: "auto" });
    } else if (clientY > rect.bottom - edge) {
      list.scrollBy({ top: 10, behavior: "auto" });
    }
  }

  function finishProfilePointerDrag(event: PointerEvent): void {
    const drag = profilePointerDrag;
    removeProfileDragListeners();
    profilePointerDrag = null;
    draggedProfileId = null;
    if (!drag?.active) return;
    event.preventDefault();
    event.stopPropagation();
    void saveProfilesOrder();
  }

  async function refreshSelectedCapturerProfileDetail(profileId: string) {
    try {
      const detail = await invoke<ProfileDetail>("get_profile_detail", {
        profileId,
      });
      if (selectedCapturerProfileId === profileId) {
        selectedCapturerProfileDetail = detail;
      }
    } catch {
      if (selectedCapturerProfileId === profileId) {
        selectedCapturerProfileDetail = null;
      }
    }
  }

  async function selectProfile(profileId: string) {
    selectedProfileId = profileId;
    await refreshProfileDetail(profileId);
  }

  async function createNewProfile() {
    if (busy) return;
    busy = true;
    message = "Creating profile...";
    errorMessage = "";
    try {
      // UN Avatar の `new_avatar_setting` と同じ UX に揃える: ユーザーに
      // ダイアログを出さず、デフォルト名 ("New Profile") で即作成 → 編集ペインへ。
      // 名前 / メモは右ペインで後から編集できる。
      const detail = await invoke<ProfileDetail>("create_profile", {
        name: "",
      });
      profileDetail = detail;
      selectedProfileId = detail.id;
      await refreshProfiles();
      message = `Created ${detail.name}`;
    } catch (error) {
      errorMessage = `create_profile failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  async function quickLaunchSelectedProfile() {
    if (busy || !selectedProfileId) return;
    launchTargetId = selectedProfileId;
    await launchCapturer();
    if (appSettings.jumpToCapturersOnQuickLaunch) {
      activeTab = "capturers";
    }
  }

  /// Profile 削除の長押し UX (UN Avatar Supervisor と同じ挙動)。`pointerdown` 中の
  /// 連続描画で `deleteHoldProgress` を 0 → 1 まで進め、1 に達したら削除を確定する。
  /// 押下中に `pointerup` / `pointerleave` / `pointercancel` でキャンセル可能。
  let deleteHoldTargetId = $state<string | null>(null);
  let deleteHoldProgress = $state(0);
  let deleteHoldTimer: number | null = null;
  let deleteHoldStartedAt = 0;
  const deleteHoldDurationMs = 1200;
  let calibratingCapturerId = $state<number | null>(null);
  let actionCountdownOverlay = $state<ActionCountdownOverlay | null>(null);

  function startDeleteHold(id: string | null): void {
    if (!id || busy) return;
    cancelDeleteHold();
    deleteHoldTargetId = id;
    deleteHoldProgress = 0;
    deleteHoldStartedAt = performance.now();
    const tick = () => {
      if (deleteHoldTargetId !== id) return;
      deleteHoldProgress = Math.min(
        1,
        (performance.now() - deleteHoldStartedAt) / deleteHoldDurationMs,
      );
      if (deleteHoldProgress >= 1) {
        cancelDeleteHold();
        void deleteSelectedProfile();
        return;
      }
      deleteHoldTimer = window.requestAnimationFrame(tick);
    };
    deleteHoldTimer = window.requestAnimationFrame(tick);
  }

  function cancelDeleteHold(): void {
    if (deleteHoldTimer != null) {
      window.cancelAnimationFrame(deleteHoldTimer);
      deleteHoldTimer = null;
    }
    deleteHoldTargetId = null;
    deleteHoldProgress = 0;
  }

  async function duplicateSelectedProfile() {
    if (busy || !selectedProfileId) return;
    busy = true;
    message = "Duplicating profile...";
    errorMessage = "";
    try {
      const detail = await invoke<ProfileDetail>("duplicate_profile", {
        profileId: selectedProfileId,
      });
      profileDetail = detail;
      selectedProfileId = detail.id;
      await refreshProfiles();
      message = `Duplicated to ${detail.name}`;
    } catch (error) {
      errorMessage = `duplicate_profile failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  async function deleteSelectedProfile() {
    if (busy || !selectedProfileId || !profileDetail) return;
    // 確認ダイアログは廃止。toolbar の Delete ボタンを 1.2 秒間長押しした場合のみ
    // この関数が呼ばれるため、誤操作の防止は UI レイヤーで担保している
    // (UN Avatar Supervisor の deleteSetting と同じ UX)。
    busy = true;
    message = "Deleting profile...";
    errorMessage = "";
    try {
      const removedId = selectedProfileId;
      await invoke("delete_profile", { profileId: removedId });
      selectedProfileId = null;
      profileDetail = null;
      await refreshProfiles();
      message = `Deleted ${removedId}`;
    } catch (error) {
      errorMessage = `delete_profile failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  async function updateProfileField(field: string, value: unknown) {
    if (!selectedProfileId) return;
    try {
      const detail = await invoke<ProfileDetail>("update_profile_field", {
        profileId: selectedProfileId,
        field,
        value,
      });
      profileDetail = detail;
      profiles = profiles.map((profile) =>
        profile.id === detail.id
          ? {
              ...profile,
              name: detail.name,
              note: detail.note,
              iconPath: detail.iconPath,
              group: detail.group,
              engine: detail.runtime.engine,
            }
          : profile,
      );
      // name 変更を一覧に即時反映するため refresh
      if (field === "name" || field === "icon_path" || field === "group") {
        await refreshProfiles();
      }
      // 編集が稼働中の Capturer に sync 反映されると、対象 profile で動く
      // runtime だけが restart する。UX を滑らかにするため telemetry を即時更新する
      // (1.5s の polling を待たない)。
      await refreshRuntimeStatuses();
    } catch (error) {
      errorMessage = `update_profile_field(${field}) failed: ${String(error)}`;
    }
  }

  async function restartCapturer(capturer: CapturerInstance) {
    if (busy) return;
    const id = capturer.id;
    const profile_id = capturer.profileId ?? null;
    if (!profile_id) {
      errorMessage = `restart capturer failed: profile is unknown for #${id}`;
      return;
    }
    busy = true;
    message = `Restarting #${id}...`;
    errorMessage = "";
    try {
      await invoke("stop_capturer", { id });
      const instance = await invoke<CapturerInstance>("launch_capturer", {
        profileId: profile_id,
        allowNonLoopback: false,
      });
      selectedCapturerId = instance.id;
      await refreshCapturers();
      message = `Restarted as #${instance.id}`;
    } catch (error) {
      errorMessage = `restart capturer failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  function emptyStringToNull(value: string | null | undefined): string | null {
    if (value === null || value === undefined) return null;
    const trimmed = value.trim();
    return trimmed === "" ? null : trimmed;
  }

  function numberOrNull(
    value: string | number | null | undefined,
  ): number | null {
    if (value === null || value === undefined || value === "") return null;
    const parsed = typeof value === "number" ? value : Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }

  type CalibrationPoseKind = "U" | "T" | "I";

  type ActionCountdownKind = "calibration" | "face-model" | "unmf-pose";

  interface ActionCountdownOverlay {
    kind: ActionCountdownKind;
    title: string;
    subtitle: string;
    imageSrc: string | null;
    remaining: number | null;
    phase: "prepare" | "sampling";
    body: string[];
    highlights: string[];
  }

  function runningCapturers(): CapturerInstance[] {
    return capturers.filter((capturer) => capturer.state === "running");
  }

  function liveCapturers(): CapturerInstance[] {
    return capturers.filter(
      (capturer) => capturer.state !== "exited" && capturer.state !== "crashed",
    );
  }

  function calibrationOverlay(
    pose: CalibrationPoseKind,
    targetLabel: string,
  ): ActionCountdownOverlay {
    const poseText = calibrationPoseDisplayLabel(pose);
    const body =
      pose === "T"
        ? [
            "実際の演者の姿勢と人型モデルの相対的な差を解消します。",
            "全身を映せる場合向けの調整用姿勢です。イラストを参考にTの字の姿勢で手首と手の平は正面に向け、親指は自然に開いた状態にして下さい。",
          ]
        : pose === "I"
          ? [
              "実際の演者の姿勢と人型モデルの相対的な差を解消します。",
              "全身を映せる場合向けの調整用姿勢です。イラストを参考にIの字の姿勢にして下さい。",
            ]
          : [
              "実際の演者の姿勢と人型モデルの相対的な差を解消します。",
              "胸上から頭部の周辺の比較的狭い映りで頭/顔/手/腕の姿勢推定に特化した調整用姿勢です。イラストを参考に手の平を正面に向け、肘は外側に残し、手首を頭上へ上げて下さい。",
            ];
    return {
      kind: "calibration",
      title: `${poseText} キャリブレーション`,
      subtitle: targetLabel,
      imageSrc:
        pose === "T" ? poseImageT : pose === "I" ? poseImageI : poseImageU,
      remaining: null,
      phase: "prepare",
      body,
      highlights:
        pose === "T"
          ? ["Tの字の姿勢", "手首と手の平は正面", "親指は自然に開く"]
          : pose === "I"
            ? ["Iの字の姿勢"]
            : ["手の平を正面", "肘は外側", "手首を頭上"],
    };
  }

  function calibrationPoseOption(
    pose: CalibrationPoseKind,
  ): (typeof calibrationPoseOptions)[number] {
    return (
      calibrationPoseOptions.find((option) => option.kind === pose) ??
      calibrationPoseOptions[0]
    );
  }

  function calibrationPoseDisplayLabel(pose: CalibrationPoseKind): string {
    return calibrationPoseOption(pose).label;
  }

  function calibrationPoseShortLabel(pose: CalibrationPoseKind): string {
    return calibrationPoseOption(pose).shortLabel;
  }

  function calibrationPoseFromRuntime(
    runtime: ProfileRuntimeView | null | undefined,
  ): CalibrationPoseKind | null {
    if (!runtime?.modifierNeutralCalibrationEnabled) return null;
    if ((runtime.modifierNeutralCalibrationSampleCount ?? 0) <= 0) return null;
    const pose = runtime.modifierNeutralCalibrationPose?.toUpperCase();
    if (pose === "T" || pose === "TWF" || pose === "T-WRIST-FRONT")
      return "T";
    if (pose === "I") return "I";
    if (pose === "U") return "U";
    return "U";
  }

  function neutralCalibrationStatusLabel(
    runtime: ProfileRuntimeView | null | undefined,
  ): string {
    const pose = calibrationPoseFromRuntime(runtime);
    if (!pose) return "No neutral calibration";
    const count = runtime?.modifierNeutralCalibrationSampleCount ?? 0;
    return `${calibrationPoseShortLabel(pose)}${count > 0 ? `: ${count} offsets` : ""}`;
  }

  function facePoseModelStatusLabel(
    runtime: ProfileRuntimeView | null | undefined,
    engine: EngineType,
  ): string {
    const unavailableReason = facePoseModelUnavailableReason(runtime, engine);
    if (unavailableReason) return unavailableReason;
    return hasFacePoseModel(runtime)
      ? `${runtime?.modifierFacePoseModelSampleCount ?? 0} samples`
      : "Not created";
  }

  function facePoseModelUnavailableReason(
    runtime: ProfileRuntimeView | null | undefined,
    engine: EngineType,
  ): string | null {
    if (!runtime) return "Loading";
    if (engine !== "mediapipe-native") return "N/A";
    const headEnabled = runtime.modifierHeadEnabled ?? true;
    const faceEnabled = runtime.modifierFaceEnabled ?? true;
    if (!headEnabled && !faceEnabled) return "Head/Face off";
    if (!headEnabled) return "Head off";
    if (!faceEnabled) return "Face off";
    if ((runtime.modifierHeadFromFaceMatrix ?? true) === false)
      return "Face Head off";
    return null;
  }

  function faceModelOverlay(targetLabel: string): ActionCountdownOverlay {
    return {
      kind: "face-model",
      title: $_("profiles.editor.face_head_model"),
      subtitle: targetLabel,
      imageSrc: poseImageI,
      remaining: null,
      phase: "prepare",
      body: [
        "Face tracking Head 推定に対して、演者固有の顔ランドマーク比率を測定し、Head の仰俯角推定を補正します。",
        "顔をカメラへ正面に向け、首を自然に立て、表情は軽くニュートラルにして下さい。作成後は Face 由来 Head の見上げ/見下げ方向と飛び耐性が改善します。",
      ],
      highlights: ["顔を正面", "首を自然に立てる", "表情はニュートラル"],
    };
  }

  function snapshotLabel(durationMs: number): string {
    if (durationMs >= 3000) return "3s Snapshot";
    if (durationMs >= 1000) return "1s Snapshot";
    return "1f Snapshot";
  }

  function snapshotKind(durationMs: number): "1f" | "1s" | "3s" {
    if (durationMs >= 3000) return "3s";
    if (durationMs >= 1000) return "1s";
    return "1f";
  }

  function unmfPoseOverlay(
    targetLabel: string,
    durationMs: number,
  ): ActionCountdownOverlay {
    const label = snapshotLabel(durationMs);
    const body =
      durationMs >= 3000
        ? "カウントダウン後に3秒間のUNMF/JSONLサンプルを保存します。動きの応答性を見たい姿勢変化を、保存開始後に行って下さい。"
        : durationMs >= 1000
          ? "カウントダウン後に1秒間のUNMF/JSONLサンプルを保存します。短い揺れや瞬間的な変化を見たい姿勢で動いて下さい。"
          : "カウントダウン後の1フレームをUNMF/JSONLとして保存します。保存したい姿勢を作って静止して下さい。";
    return {
      kind: "unmf-pose",
      title: `${label} 保存`,
      subtitle: targetLabel,
      imageSrc: null,
      remaining: null,
      phase: "prepare",
      body: [body],
      highlights:
        durationMs >= 3000
          ? ["保存開始後に動かす", "3秒間サンプリング"]
          : durationMs >= 1000
            ? ["保存開始後に動かす", "1秒間サンプリング"]
            : ["保存したい姿勢で静止"],
    };
  }

  async function runActionCountdown(
    label: string,
    samplingLabel = "sampling",
    overlay: ActionCountdownOverlay | null = null,
  ): Promise<void> {
    const delay = Math.max(
      0,
      Math.min(30, Math.round(appSettings.calibrationStartDelaySeconds ?? 3)),
    );
    if (overlay) {
      actionCountdownOverlay = {
        ...overlay,
        remaining: delay > 0 ? delay : null,
        phase: "prepare",
      };
    }
    for (let remaining = delay; remaining >= 1; remaining -= 1) {
      message = `${label}: ${remaining}s`;
      if (actionCountdownOverlay) {
        actionCountdownOverlay = {
          ...actionCountdownOverlay,
          remaining,
          phase: "prepare",
        };
      }
      await playCalibrationSound(
        appSettings.calibrationCountdownSoundPath,
        defaultCalibrationCountdownSound,
      );
      await sleepMs(1000);
    }
    message = `${label}: ${samplingLabel}...`;
    if (actionCountdownOverlay) {
      actionCountdownOverlay = {
        ...actionCountdownOverlay,
        remaining: null,
        phase: "sampling",
      };
    }
    await playCalibrationSound(
      appSettings.calibrationStartSoundPath,
      defaultCalibrationStartSound,
    );
  }

  async function runCalibrationCountdown(
    pose: CalibrationPoseKind,
    capturerId: number,
  ): Promise<void> {
    const poseLabel = calibrationPoseDisplayLabel(pose);
    await runActionCountdown(
      `${poseLabel} calibration #${capturerId}`,
      "sampling",
      calibrationOverlay(pose, `Capturer #${capturerId}`),
    );
  }

  async function invokeCapturerNeutralCalibration(
    capturer: CapturerInstance,
    pose: CalibrationPoseKind,
  ): Promise<ProfileDetail> {
    return invoke<ProfileDetail>("calibrate_capturer_neutral", {
      id: capturer.id,
      pose,
      validSampleCount: Math.max(
        1,
        Math.min(240, Math.round(appSettings.calibrationSampleCount ?? 45)),
      ),
    });
  }

  async function calibrateCapturerNeutral(
    capturer: CapturerInstance,
    pose: CalibrationPoseKind,
  ): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    calibratingCapturerId = capturer.id;
    const poseLabel = calibrationPoseDisplayLabel(pose);
    message = `${poseLabel} calibration #${capturer.id}: prepare pose...`;
    errorMessage = "";
    try {
      await runCalibrationCountdown(pose, capturer.id);
      const detail = await invokeCapturerNeutralCalibration(capturer, pose);
      if (profileDetail?.id === detail.id || selectedProfileId === detail.id) {
        profileDetail = detail;
      }
      if (selectedCapturerProfileId === detail.id) {
        selectedCapturerProfileDetail = detail;
      }
      await refreshProfiles();
      await refreshRuntimeStatuses();
      message = `${poseLabel} calibration saved for ${detail.name}`;
    } catch (error) {
      errorMessage = `neutral calibration failed: ${String(error)}`;
      message = "";
    } finally {
      actionCountdownOverlay = null;
      calibratingCapturerId = null;
    }
  }

  async function clearCapturerNeutralCalibration(
    capturer: CapturerInstance,
  ): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    calibratingCapturerId = capturer.id;
    message = `Clearing calibration #${capturer.id}...`;
    errorMessage = "";
    try {
      const detail = await invoke<ProfileDetail>(
        "clear_capturer_neutral_calibration",
        {
          id: capturer.id,
        },
      );
      if (profileDetail?.id === detail.id || selectedProfileId === detail.id) {
        profileDetail = detail;
      }
      if (selectedCapturerProfileId === detail.id) {
        selectedCapturerProfileDetail = detail;
      }
      await refreshProfiles();
      await refreshRuntimeStatuses();
      message = `Calibration cleared for ${detail.name}`;
    } catch (error) {
      errorMessage = `clear calibration failed: ${String(error)}`;
      message = "";
    } finally {
      calibratingCapturerId = null;
    }
  }

  async function buildCapturerFacePoseModel(
    capturer: CapturerInstance,
  ): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    calibratingCapturerId = capturer.id;
    message = `Face Head personal correction #${capturer.id}: face front...`;
    errorMessage = "";
    try {
      await runActionCountdown(
        `Face Head personal correction #${capturer.id}`,
        "sampling",
        faceModelOverlay(`Capturer #${capturer.id}`),
      );
      const detail = await invoke<ProfileDetail>(
        "build_capturer_face_pose_model",
        {
          id: capturer.id,
          validSampleCount: Math.max(
            1,
            Math.min(240, Math.round(appSettings.calibrationSampleCount ?? 90)),
          ),
        },
      );
      if (profileDetail?.id === detail.id || selectedProfileId === detail.id) {
        profileDetail = detail;
      }
      await refreshProfiles();
      await refreshRuntimeStatuses();
      const neutral =
        detail.runtime.modifierFacePoseModelNeutralNoseDropEyeMouth;
      message = `Face Head personal correction saved for ${detail.name}${neutral == null ? "" : ` (${neutral.toFixed(3)})`}`;
    } catch (error) {
      errorMessage = `Face Head personal correction build failed: ${String(error)}`;
      message = "";
    } finally {
      actionCountdownOverlay = null;
      calibratingCapturerId = null;
    }
  }

  async function clearCapturerFacePoseModel(
    capturer: CapturerInstance,
  ): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    const profileId = capturer.profileId;
    if (!profileId) {
      errorMessage = `clear Face Head personal correction failed: profile is unknown for #${capturer.id}`;
      return;
    }
    busy = true;
    message = `Clearing Face Head personal correction #${capturer.id}...`;
    errorMessage = "";
    try {
      const detail = await invoke<ProfileDetail>("update_profile_field", {
        profileId,
        field: "runtime_selection.modifier.face_pose_model.enabled",
        value: false,
      });
      if (profileDetail?.id === detail.id || selectedProfileId === detail.id) {
        profileDetail = detail;
      }
      if (selectedCapturerProfileId === detail.id) {
        selectedCapturerProfileDetail = detail;
      }
      await refreshProfiles();
      await refreshRuntimeStatuses();
      message = `Face Head personal correction cleared for ${detail.name}`;
    } catch (error) {
      errorMessage = `clear Face Head personal correction failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  async function buildSelectedProfileFacePoseModel(): Promise<void> {
    if (busy || calibratingCapturerId != null || !profileDetail) return;
    const profileId = profileDetail.id;
    const profileName = profileDetail.name;
    await refreshCapturers();
    const existing = capturers.find(
      (capturer) =>
        capturer.profileId === profileId &&
        (capturer.state === "running" || capturer.state === "starting"),
    );
    let capturer = existing ?? null;
    let shouldStopAfterBuild = false;
    busy = true;
    message = existing
      ? `Face Head personal correction #${existing.id}: face front...`
      : `Launching ${profileName} for Face Head personal correction...`;
    errorMessage = "";
    try {
      if (!capturer) {
        capturer = await invoke<CapturerInstance>("launch_capturer", {
          profileId,
          allowNonLoopback: false,
        });
        shouldStopAfterBuild = true;
        selectedCapturerId = capturer.id;
        await refreshCapturers();
      }
      calibratingCapturerId = capturer.id;
      await runActionCountdown(
        `Face Head personal correction #${capturer.id}`,
        "sampling",
        faceModelOverlay(`${profileName} / Capturer #${capturer.id}`),
      );
      const detail = await invoke<ProfileDetail>(
        "build_capturer_face_pose_model",
        {
          id: capturer.id,
          validSampleCount: Math.max(
            1,
            Math.min(240, Math.round(appSettings.calibrationSampleCount ?? 90)),
          ),
        },
      );
      profileDetail = detail;
      selectedProfileId = detail.id;
      await refreshProfiles();
      await refreshRuntimeStatuses();
      const neutral =
        detail.runtime.modifierFacePoseModelNeutralNoseDropEyeMouth;
      message = `Face Head personal correction saved for ${detail.name}${neutral == null ? "" : ` (${neutral.toFixed(3)})`}`;
    } catch (error) {
      errorMessage = `Face Head personal correction build failed: ${String(error)}`;
      message = "";
    } finally {
      actionCountdownOverlay = null;
      calibratingCapturerId = null;
      if (shouldStopAfterBuild && capturer) {
        try {
          await invoke("stop_capturer", { id: capturer.id });
          await refreshCapturers();
        } catch (error) {
          errorMessage = `Face Head personal correction saved, but temporary capturer stop failed: ${String(error)}`;
        }
      }
      busy = false;
    }
  }

  async function saveCapturerUnmfPose(
    capturer: CapturerInstance,
    durationMs = 0,
  ): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    calibratingCapturerId = capturer.id;
    const label = snapshotLabel(durationMs);
    const kind = snapshotKind(durationMs);
    message = `${label} #${capturer.id}: prepare pose...`;
    errorMessage = "";
    try {
      await runActionCountdown(
        `${label} #${capturer.id}`,
        "capturing",
        unmfPoseOverlay(`Capturer #${capturer.id}`, durationMs),
      );
      await invoke<string | null>("save_capturer_unmf_pose", {
        id: capturer.id,
        durationMs,
        snapshotKind: kind,
      });
      message = "";
    } catch (error) {
      errorMessage = `save ${label} failed: ${String(error)}`;
      message = "";
    } finally {
      actionCountdownOverlay = null;
      calibratingCapturerId = null;
    }
  }

  async function saveAllCapturersUnmfPose(durationMs = 0): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    const targets = runningCapturers();
    if (targets.length === 0) return;
    busy = true;
    const label = snapshotLabel(durationMs);
    const kind = snapshotKind(durationMs);
    message = `${label} ALL: prepare pose...`;
    errorMessage = "";
    try {
      await runActionCountdown(
        `${label} ALL`,
        "capturing",
        unmfPoseOverlay(`${targets.length} Capturers`, durationMs),
      );
      await invoke<string | null>("save_all_capturers_unmf_pose", {
        durationMs,
        snapshotKind: kind,
      });
      message = "";
    } catch (error) {
      errorMessage = `save ALL ${label} failed: ${String(error)}`;
      message = "";
    } finally {
      actionCountdownOverlay = null;
      busy = false;
    }
  }

  async function clearAllCapturerNeutralCalibrations(): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    const targets = runningCapturers();
    if (targets.length === 0) return;
    busy = true;
    message = `Clearing calibration for ${targets.length} capturers...`;
    errorMessage = "";
    try {
      let lastDetail: ProfileDetail | null = null;
      for (const capturer of targets) {
        lastDetail = await invoke<ProfileDetail>(
          "clear_capturer_neutral_calibration",
          { id: capturer.id },
        );
        if (
          profileDetail?.id === lastDetail.id ||
          selectedProfileId === lastDetail.id
        ) {
          profileDetail = lastDetail;
        }
      }
      await refreshProfiles();
      await refreshRuntimeStatuses();
      message = `Calibration cleared for ${targets.length} capturers`;
    } catch (error) {
      errorMessage = `clear ALL calibration failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  async function calibrateAllCapturersNeutral(
    pose: CalibrationPoseKind,
  ): Promise<void> {
    if (busy || calibratingCapturerId != null) return;
    const targets = runningCapturers();
    if (targets.length === 0) return;
    busy = true;
    const poseLabel = calibrationPoseDisplayLabel(pose);
    message = `${poseLabel} calibration ALL: prepare pose...`;
    errorMessage = "";
    try {
      await runActionCountdown(
        `${poseLabel} calibration ALL`,
        "sampling",
        calibrationOverlay(pose, `${targets.length} Capturers`),
      );
      let lastDetail: ProfileDetail | null = null;
      for (const capturer of targets) {
        lastDetail = await invokeCapturerNeutralCalibration(capturer, pose);
        if (
          profileDetail?.id === lastDetail.id ||
          selectedProfileId === lastDetail.id
        ) {
          profileDetail = lastDetail;
        }
      }
      await refreshProfiles();
      await refreshRuntimeStatuses();
      message = `${poseLabel} calibration saved for ${targets.length} capturers`;
    } catch (error) {
      errorMessage = `calibrate ALL ${poseLabel} failed: ${String(error)}`;
      message = "";
    } finally {
      actionCountdownOverlay = null;
      busy = false;
    }
  }

  function smoothingEmaEnabled(runtime: ProfileRuntimeView): boolean {
    if (runtime.modifierSmoothingEmaEnabled !== null)
      return runtime.modifierSmoothingEmaEnabled;
    return ["low", "medium", "high"].includes(
      runtime.modifierSmoothingPreset ?? "",
    );
  }

  function smoothingOneEuroEnabled(runtime: ProfileRuntimeView): boolean {
    if (runtime.modifierSmoothingOneEuroEnabled !== null)
      return runtime.modifierSmoothingOneEuroEnabled;
    return runtime.modifierSmoothingPreset === "adaptive";
  }

  function smoothingEmaAlpha(runtime: ProfileRuntimeView): number {
    if (runtime.modifierSmoothingEmaAlpha !== null)
      return runtime.modifierSmoothingEmaAlpha;
    if (runtime.modifierSmoothingPreset === "low") return 0.7;
    if (runtime.modifierSmoothingPreset === "high") return 0.25;
    return 0.45;
  }

  function torsoPitchScale(runtime: ProfileRuntimeView): number {
    return clampNumber(runtime.modifierTorsoPitchScale ?? 1.0, 0.0, 1.0);
  }

  function applyEmaRecommendation(alpha: number): void {
    void updateProfileFields({
      "runtime_selection.modifier.smoothing_ema_enabled": true,
      "runtime_selection.modifier.smoothing_ema_alpha": alpha,
    });
  }

  function applyOneEuroRecommendation(
    minCutoffHz: number,
    beta: number,
    derivativeCutoffHz: number,
  ): void {
    void updateProfileFields({
      "runtime_selection.modifier.smoothing_one_euro_enabled": true,
      "runtime_selection.modifier.smoothing_confidence_adaptive_cutoff": true,
      "runtime_selection.modifier.adaptive_min_cutoff_hz": minCutoffHz,
      "runtime_selection.modifier.adaptive_beta": beta,
      "runtime_selection.modifier.adaptive_derivative_cutoff_hz":
        derivativeCutoffHz,
    });
  }

  function clampNumber(value: number, min: number, max: number): number {
    return Math.min(max, Math.max(min, value));
  }

  function lostSignalBehavior(runtime: ProfileRuntimeView): string {
    const value = runtime.modifierLostSignalBehavior ?? "rest-pose";
    if (value === "hold" || value === "drop" || value === "rest-pose")
      return value;
    return "rest-pose";
  }

  function lostSignalRestPoseBlend(runtime: ProfileRuntimeView): number {
    return clampNumber(runtime.modifierLostSignalRestPoseBlend ?? 0.3, 0, 1);
  }

  function lostSignalHoldSeconds(runtime: ProfileRuntimeView): number {
    return clampNumber(runtime.modifierLostSignalHoldSeconds ?? 8.2, 0, 30);
  }

  type LostSignalPart = "head" | "hands" | "arms";
  const lostSignalParts: LostSignalPart[] = ["head", "hands", "arms"];

  function lostSignalPartLabel(part: LostSignalPart): string {
    if (part === "head") return "Head";
    if (part === "hands") return "Hands";
    return "Arms";
  }

  function lostSignalPartBehavior(
    runtime: ProfileRuntimeView,
    part: LostSignalPart,
  ): string {
    const value =
      part === "head"
        ? runtime.modifierLostSignalHeadBehavior
        : part === "hands"
          ? runtime.modifierLostSignalHandsBehavior
          : runtime.modifierLostSignalArmsBehavior;
    if (value === "hold" || value === "drop" || value === "rest-pose")
      return value;
    return part === "head" ? "hold" : "rest-pose";
  }

  function lostSignalPartRestPoseBlend(
    runtime: ProfileRuntimeView,
    part: LostSignalPart,
  ): number {
    const value =
      part === "head"
        ? runtime.modifierLostSignalHeadRestPoseBlend
        : part === "hands"
          ? runtime.modifierLostSignalHandsRestPoseBlend
          : runtime.modifierLostSignalArmsRestPoseBlend;
    return clampNumber(value ?? 0.3, 0, 1);
  }

  function lostSignalPartHoldSeconds(
    runtime: ProfileRuntimeView,
    part: LostSignalPart,
  ): number {
    const value =
      part === "head"
        ? runtime.modifierLostSignalHeadHoldSeconds
        : part === "hands"
          ? runtime.modifierLostSignalHandsHoldSeconds
          : runtime.modifierLostSignalArmsHoldSeconds;
    return clampNumber(value ?? 8.2, 0, 30);
  }

  function lostSignalPartField(part: LostSignalPart, suffix: string): string {
    const prefix =
      part === "head" ? "head" : part === "hands" ? "hands" : "arms";
    return `runtime_selection.modifier.post_process_rules.lost_signal_${prefix}_${suffix}`;
  }

  function lostSignalRecoverySeconds(runtime: ProfileRuntimeView): number {
    return clampNumber(runtime.modifierLostSignalRecoverySeconds ?? 0.25, 0, 5);
  }

  function formatRatio(value: number): string {
    return value.toFixed(2).replace(/0$/, "").replace(/\.0$/, "");
  }

  function statusLabel(status: string): string {
    if (status === "estimated") return "推定";
    if (status === "held") return "ホールド";
    if (status === "recovering") return "復帰中";
    if (status === "off") return "OFF";
    return "ロスト";
  }

  function partEnabled(
    runtime: ProfileRuntimeView | null | undefined,
    group: string,
  ): boolean {
    if (!runtime) return true;
    if (group === "Head") return runtime.modifierHeadEnabled ?? true;
    if (group === "Hands" || group === "L-Hand" || group === "R-Hand")
      return runtime.modifierHandsEnabled ?? true;
    if (group === "Arms" || group === "L-Arm" || group === "R-Arm")
      return runtime.modifierArmsIkEnabled ?? true;
    if (group === "Torso") return runtime.modifierTorsoEnabled ?? false;
    if (group === "Legs" || group === "L-Leg" || group === "R-Leg")
      return runtime.modifierLegsEnabled ?? false;
    if (group === "Feet" || group === "L-Foot" || group === "R-Foot")
      return runtime.modifierFeetEnabled ?? false;
    return true;
  }

  function diagnosticForPart(
    snapshot: Record<string, unknown> | null | undefined,
    runtime: ProfileRuntimeView | null | undefined,
    group: string,
  ): PartDiagnostic {
    const canonical = diagnosticGroupKey(group);
    const fallback = diagnosticGroupFallbackKey(canonical);
    if (!partEnabled(runtime, group))
      return { group: canonical, status: "off", confidence: 0 };
    const direct = diagnosticFromMap(
      (
        snapshot?.composed as
          | { part_diagnostics?: Record<string, PartDiagnostic> }
          | null
          | undefined
      )?.part_diagnostics,
      canonical,
    );
    if (direct) return direct;
    const fallbackDirect = diagnosticFromMap(
      (
        snapshot?.composed as
          | { part_diagnostics?: Record<string, PartDiagnostic> }
          | null
          | undefined
      )?.part_diagnostics,
      fallback,
    );
    if (fallbackDirect) return fallbackDirect;
    const streams =
      (snapshot?.streams as
        | { part_diagnostics?: Record<string, PartDiagnostic> }[]
        | null
        | undefined) ?? [];
    for (const stream of streams) {
      const fromStream = diagnosticFromMap(stream.part_diagnostics, canonical);
      if (fromStream) return fromStream;
      const fromFallbackStream = diagnosticFromMap(
        stream.part_diagnostics,
        fallback,
      );
      if (fromFallbackStream) return fromFallbackStream;
    }
    return { group: canonical, status: "lost", confidence: 0 };
  }

  function diagnosticGroupKey(group: string): string {
    if (group === "L-Hand") return "LeftHand";
    if (group === "R-Hand") return "RightHand";
    if (group === "L-Arm") return "LeftArm";
    if (group === "R-Arm") return "RightArm";
    if (group === "L-Leg") return "LeftLeg";
    if (group === "R-Leg") return "RightLeg";
    if (group === "L-Foot") return "LeftFoot";
    if (group === "R-Foot") return "RightFoot";
    const sideStripped = group.replace(/^L-/, "").replace(/^R-/, "");
    if (sideStripped === "Hand") return "Hands";
    if (sideStripped === "Arm") return "Arms";
    if (sideStripped === "Leg") return "Legs";
    if (sideStripped === "Foot") return "Feet";
    return sideStripped;
  }

  function diagnosticGroupFallbackKey(canonical: string): string {
    if (canonical === "LeftHand" || canonical === "RightHand") return "Hands";
    if (canonical === "LeftArm" || canonical === "RightArm") return "Arms";
    if (canonical === "LeftLeg" || canonical === "RightLeg") return "Legs";
    if (canonical === "LeftFoot" || canonical === "RightFoot") return "Feet";
    return canonical;
  }

  function diagnosticFromMap(
    diagnostics: Record<string, PartDiagnostic> | null | undefined,
    group: string,
  ): PartDiagnostic | null {
    if (!diagnostics) return null;
    return (
      diagnostics[group] ??
      diagnostics[group.toLowerCase()] ??
      diagnostics[group.charAt(0).toLowerCase() + group.slice(1)] ??
      null
    );
  }

  function diagnosticClass(diagnostic: PartDiagnostic): string {
    if (diagnostic.status === "off") return "diag-off";
    if (diagnostic.status === "lost") return "diag-lost";
    if (diagnostic.status === "held" || diagnostic.status === "recovering")
      return "diag-held";
    if (diagnostic.confidence < 0.35) return "diag-low";
    if (diagnostic.confidence < 0.75) return "diag-mid";
    return "diag-high";
  }

  function confidencePercent(diagnostic: PartDiagnostic): number {
    return Math.round(clampNumber(diagnostic.confidence ?? 0, 0, 1) * 100);
  }

  function sourceRateForPart(
    fps: CapturerOutputFps | null | undefined,
    engine: EngineType,
    group: string,
  ): string | null {
    if (!fps) return null;
    if (engine === "vmc") {
      const rate = fps.vmcPacketsPerSec || fps.vmcDatagramsPerSec;
      return rate > 0 ? formatTelemetryWithUnit(rate, "pkt/s") : null;
    }
    if (engine === "ifacialmocap" && (group === "Head" || group === "Face")) {
      const rate = bestSourceRate(fps);
      return rate != null && rate > 0
        ? formatTelemetryWithUnit(rate, "pkt/s")
        : null;
    }
    return null;
  }

  function profileNeedsFacePoseModel(
    runtime: ProfileRuntimeView | null | undefined,
    engine: EngineType,
  ): boolean {
    if (!runtime) return false;
    if (!canUseFacePoseModel(runtime, engine)) return false;
    return !(
      runtime.modifierFacePoseModelEnabled &&
      runtime.modifierFacePoseModelNeutralNoseDropEyeMouth != null
    );
  }

  function canUseFacePoseModel(
    runtime: ProfileRuntimeView | null | undefined,
    engine: EngineType,
  ): boolean {
    return facePoseModelUnavailableReason(runtime, engine) == null;
  }

  function hasFacePoseModel(
    runtime: ProfileRuntimeView | null | undefined,
  ): boolean {
    return Boolean(
      runtime?.modifierFacePoseModelEnabled &&
        runtime.modifierFacePoseModelNeutralNoseDropEyeMouth != null,
    );
  }

  function summaryNeedsFacePoseModel(
    profile: ProfileSummary | null | undefined,
    engine: EngineType,
  ): boolean {
    if (!profile) return false;
    if (engine !== "mediapipe-native") return false;
    const modifier = profile.runtimeSelection?.modifier;
    if ((modifier?.headEnabled ?? true) === false) return false;
    if ((modifier?.faceEnabled ?? true) === false) return false;
    const model = modifier?.facePoseModel;
    return !(model?.enabled && model.neutralNoseDropEyeMouth != null);
  }

  function profileSummaryResolution(
    profile: ProfileSummary | null | undefined,
  ): string {
    const runtime = profile?.runtimeSelection;
    const pipeline = profile?.pipelineComponents;
    if (runtime?.resolution) return runtime.resolution;
    if (pipeline?.inputWidth && pipeline?.inputHeight)
      return `${pipeline.inputWidth}x${pipeline.inputHeight}`;
    return "-";
  }

  function profileSummaryInputPixelFormat(
    profile: ProfileSummary | null | undefined,
  ): string {
    return profile?.pipelineComponents?.inputPixelFormat ?? "auto";
  }

  function profileSummarySourceLine(
    profile: ProfileSummary | null | undefined,
  ): string {
    const engine = normalizeEngineType(
      profile?.engine ?? profile?.runtimeSelection?.engine ?? null,
    );
    if (engine === "vmc") {
      const listen = profile?.runtimeSelection?.vmcReceiveListenAddr;
      return listen?.trim() ? `VMC UDP ${listen}` : "VMC receive (UDP)";
    }
    if (engine === "ifacialmocap") {
      const listen = profile?.runtimeSelection?.ifacialmocapReceiveListenAddr;
      return listen?.trim()
        ? `iFacialMocap UDP ${listen}`
        : "iFacialMocap receive (UDP)";
    }
    return `${profileSummaryResolution(profile)} ${profileSummaryInputPixelFormat(profile)}/RGB`;
  }

  function profileSummaryOutputFps(
    profile: ProfileSummary | null | undefined,
  ): number | null {
    return (
      profile?.runtimeSelection?.fps ??
      profile?.pipelineComponents?.inputFps ??
      null
    );
  }

  async function updateProfileFields(
    fields: Record<string, unknown>,
  ): Promise<void> {
    if (!selectedProfileId) return;
    try {
      let detail: ProfileDetail | null = null;
      for (const [field, value] of Object.entries(fields)) {
        detail = await invoke<ProfileDetail>("update_profile_field", {
          profileId: selectedProfileId,
          field,
          value,
        });
      }
      if (detail) {
        profileDetail = detail;
      }
      await refreshRuntimeStatuses();
    } catch (error) {
      errorMessage = `update profile fields failed: ${String(error)}`;
    }
  }

  async function browseInputFile(): Promise<void> {
    if (!selectedProfileId) return;
    const kind =
      mediaPipeSourceType === "file-video" ? "file-video" : "file-image";
    try {
      const path = await invoke<string | null>("pick_file_path", { kind });
      if (path == null) return;
      if (kind === "file-video") {
        await updateVideoInputPath(path);
      } else {
        await updateProfileField("pipeline_components.input_path", path);
      }
    } catch (error) {
      errorMessage = `pick input file failed: ${String(error)}`;
    }
  }

  async function updateVideoInputPath(path: string | null): Promise<void> {
    if (!path) {
      await updateProfileFields({
        "pipeline_components.input_path": null,
        "pipeline_components.input_fps": null,
        "pipeline_components.input_width": null,
        "pipeline_components.input_height": null,
      });
      return;
    }
    const fields: Record<string, unknown> = {
      "pipeline_components.input_path": path,
    };
    try {
      const meta = await invoke<VideoFileMetadata>(
        "probe_video_file_metadata",
        {
          path,
          ffmpegPath: appSettings.externalToolsFfmpegPath ?? null,
        },
      );
      fields["pipeline_components.input_fps"] = meta.fpsRounded ?? null;
      fields["pipeline_components.input_width"] = meta.width ?? null;
      fields["pipeline_components.input_height"] = meta.height ?? null;
      const fpsLabel =
        meta.fps != null
          ? meta.fps.toFixed(3).replace(/\.?0+$/, "")
          : "unknown";
      message = `Video metadata: ${meta.width ?? "?"}x${meta.height ?? "?"} @ ${fpsLabel} fps`;
    } catch (error) {
      fields["pipeline_components.input_fps"] = null;
      errorMessage = `video metadata probe failed: ${String(error)}`;
    }
    await updateProfileFields(fields);
  }

  function webcamFormatValue(format: WebcamFormatDto): string {
    const fps = format.fps ?? "auto";
    return `${format.width}x${format.height}@${fps} ${format.pixelFormat}`;
  }

  function selectedCameraSettingValue(detail: ProfileDetail): string {
    const resolution = detail.runtime.resolution ?? "";
    if (!resolution) return "";
    const fps = detail.pipeline.inputFps ?? null;
    const pixelFormat = detail.pipeline.inputPixelFormat ?? null;
    const exact = webcamFormats.find(
      (format) =>
        `${format.width}x${format.height}` === resolution &&
        (fps == null || format.fps === fps) &&
        (pixelFormat == null || format.pixelFormat === pixelFormat),
    );
    return exact ? webcamFormatValue(exact) : resolution;
  }

  async function updateCameraSetting(raw: string): Promise<void> {
    if (!selectedProfileId) return;
    if (!raw) {
      await updateProfileFields({
        "runtime_selection.resolution": null,
        "runtime_selection.fps": null,
        "pipeline_components.input_fps": null,
        "pipeline_components.input_pixel_format": null,
      });
      return;
    }
    const selected = webcamFormats.find(
      (format) => webcamFormatValue(format) === raw,
    );
    if (selected) {
      await updateProfileFields({
        "pipeline_components.input_fps": selected.fps ?? null,
        "pipeline_components.input_pixel_format": selected.pixelFormat,
        "runtime_selection.resolution": `${selected.width}x${selected.height}`,
        "runtime_selection.fps": null,
      });
      return;
    }
    await invoke("update_profile_field", {
      profileId: selectedProfileId,
      field: "pipeline_components.input_pixel_format",
      value: null,
    });
    await updateProfileField("runtime_selection.resolution", raw);
  }

  /// 表示・保存上の Engine Type を解決する。
  /// ``runtime_selection.engine`` を読み取り、未指定なら ``mediapipe-native`` を既定値とする。
  function resolveEngineType(detail: ProfileDetail | null): EngineType {
    if (!detail) return "mediapipe-native";
    const candidate = detail.runtime.engine ?? "";
    return normalizeEngineType(candidate);
  }

  function normalizeEngineType(
    candidate: string | null | undefined,
  ): EngineType {
    const raw = candidate ?? "";
    switch (raw) {
      case "mediapipe-native":
      case "vmc":
      case "ifacialmocap":
        return raw;
      default:
        return "mediapipe-native";
    }
  }

  /// Capturer 一覧のプロファイル列など、Engine の raw id を Editor の選択肢と同じ表記に寄せる。
  function engineTypeListLabel(engine: string | null | undefined): string {
    if (engine == null || !String(engine).trim()) return "—";
    const e = String(engine).trim();
    const map: Record<string, string> = {
      "mediapipe-native": "MediaPipe Native (webcam / file)",
      vmc: "VMC receive (UDP)",
      ifacialmocap: "iFacialMocap receive (UDP)",
    };
    return map[e] ?? e;
  }

  /// MediaPipe Engine 系における Source Type を解決する。``pipeline_components.input``
  /// を読む。MediaPipe 系以外の Engine では意味を持たないので、呼び出し側で Engine Type を
  /// 先に判別して使うこと。
  function resolveMediaPipeSourceType(
    detail: ProfileDetail | null,
  ): MediaPipeSourceType {
    const candidate = detail?.pipeline.input ?? "";
    switch (candidate) {
      case "webcam-directshow":
      case "webcam-mediafoundation":
      case "file-image":
      case "file-video":
        return candidate;
      // 旧 nokhwa 表記は GUI 上では MediaFoundation として扱う (実装詳細を隠す)。
      case "webcam-nokhwa":
        return "webcam-mediafoundation";
      default:
        return "webcam-directshow";
    }
  }

  /// Engine Type の変更を一括反映する。
  /// VMC / iFacialMocap engine では MediaPipe webcam 用フィールドも同時にクリアして
  /// 不釣り合いなフィールド残留 (古い設定の引きずり) を防ぐ。
  async function updateEngineType(newType: EngineType): Promise<void> {
    if (!selectedProfileId) return;
    try {
      // VMC / iFacialMocap に切り替えるときは MediaPipe webcam 関連フィールドもクリア。
      if (newType === "vmc" || newType === "ifacialmocap") {
        await invoke("update_profile_field", {
          profileId: selectedProfileId,
          field: "runtime_selection.device",
          value: null,
        });
        await invoke("update_profile_field", {
          profileId: selectedProfileId,
          field: "runtime_selection.resolution",
          value: null,
        });
        await invoke("update_profile_field", {
          profileId: selectedProfileId,
          field: "pipeline_components.input",
          value: null,
        });
        await invoke("update_profile_field", {
          profileId: selectedProfileId,
          field: "pipeline_components.input_path",
          value: null,
        });
      }
      // MediaPipe 系に切り替えるときは VMC / iFacialMocap listen address をクリア。
      if (newType === "mediapipe-native") {
        await invoke("update_profile_field", {
          profileId: selectedProfileId,
          field: "runtime_selection.vmc_receive_listen_addr",
          value: null,
        });
        await invoke("update_profile_field", {
          profileId: selectedProfileId,
          field: "runtime_selection.ifacialmocap_receive_listen_addr",
          value: null,
        });
      }
      // 最後に engine 自体を更新 (これで profileDetail も refresh される)。
      await updateProfileField("runtime_selection.engine", newType);
    } catch (error) {
      errorMessage = `update engine type failed: ${String(error)}`;
    }
  }

  /// MediaPipe Source Type の変更を ``pipeline_components.input`` に書き込む。
  /// Source Type 切替に伴う冪等な fallback クリア (file-image に切り替えたら video 専用設定を
  /// クリアするか等) は今のところしていない。空欄なら GUI 側で表示されないので実害は小さい。
  async function updateMediaPipeSourceType(
    newType: MediaPipeSourceType,
  ): Promise<void> {
    await updateProfileField("pipeline_components.input", newType);
  }

  // ===================================================================
  // Webcam device / format の列挙。
  //
  // Tauri backend (`enumerate_webcams` / `enumerate_webcam_formats`) を呼び、
  // 選択中 Source Type に対応するデバイス一覧 + そのデバイスで使える
  // (width x height @ fps  pixel_format) の組み合わせを取得して dropdown に
  // 表示する。実装詳細 (nokhwa の利用) は GUI から見えないように "Webcam
  // (MediaFoundation)" として束ねる (ユーザー要望)。
  // ===================================================================

  let webcamDevices = $state<WebcamDeviceDto[]>([]);
  let webcamDevicesLoading = $state(false);
  let webcamDevicesError = $state("");
  let webcamFormats = $state<WebcamFormatDto[]>([]);
  let webcamFormatsLoading = $state(false);
  let webcamFormatsError = $state("");

  function currentWebcamBackend(): "directshow" | "mediafoundation" | null {
    if (mediaPipeSourceType === "webcam-directshow") return "directshow";
    if (mediaPipeSourceType === "webcam-mediafoundation")
      return "mediafoundation";
    return null;
  }

  async function refreshWebcamDevices() {
    const backend = currentWebcamBackend();
    if (!backend) {
      webcamDevices = [];
      webcamDevicesError = "";
      return;
    }
    webcamDevicesLoading = true;
    webcamDevicesError = "";
    try {
      webcamDevices = await invoke<WebcamDeviceDto[]>("enumerate_webcams", {
        backend,
      });
    } catch (error) {
      webcamDevicesError = String(error);
      webcamDevices = [];
    } finally {
      webcamDevicesLoading = false;
    }
  }

  async function refreshWebcamFormats(deviceIdOrName: string) {
    const backend = currentWebcamBackend();
    if (!backend || deviceIdOrName.trim() === "") {
      webcamFormats = [];
      webcamFormatsError = "";
      return;
    }
    webcamFormatsLoading = true;
    webcamFormatsError = "";
    try {
      webcamFormats = await invoke<WebcamFormatDto[]>(
        "enumerate_webcam_formats",
        { backend, deviceIdOrName },
      );
    } catch (error) {
      webcamFormatsError = String(error);
      webcamFormats = [];
    } finally {
      webcamFormatsLoading = false;
    }
  }

  // mediaPipeSourceType (webcam-* / file-*) が変わったらデバイス一覧を再取得する。
  // file-image / file-video の場合はクリア (currentWebcamBackend が null)。
  $effect(() => {
    void refreshWebcamDevices();
  });

  // 選択中 Camera device が変わったら、そのデバイスで利用可能な format 一覧を
  // 取得して resolution / fps dropdown を構築する。DirectShow と
  // MediaFoundation の両方に対応。MediaFoundation は他アプリが
  // カメラを占有していると Err になる仕様で、その場合は GUI 側で
  // webcamFormatsError を hint として表示し、フリーフォーム入力欄に
  // fallback する (`refreshWebcamFormats` 内で webcamFormats = [] となる)。
  $effect(() => {
    const device = profileDetail?.runtime.device ?? "";
    void refreshWebcamFormats(device);
  });

  async function refreshRuntimeStatuses() {
    const updates: Record<number, CapturerRuntimeStatus> = {};
    for (const capturer of capturers) {
      if (capturer.state !== "running" && capturer.state !== "starting") {
        continue;
      }
      try {
        updates[capturer.id] = await invoke<CapturerRuntimeStatus>(
          "capturer_runtime_status",
          { id: capturer.id },
        );
      } catch {
        // 個別 Capturer の telemetry 失敗は GUI 全体を壊さない。
      }
    }
    runtimeStatuses = { ...runtimeStatuses, ...updates };
  }

  async function launchCapturer() {
    if (busy) return;
    busy = true;
    message = "Launching capturer...";
    errorMessage = "";
    try {
      if (launchGroupName) {
        if (launchGroupProfiles.length === 0) {
          throw new Error(`No profiles in group: ${launchGroupName}`);
        }
        let lastInstance: CapturerInstance | null = null;
        for (const profile of launchGroupProfiles) {
          lastInstance = await invoke<CapturerInstance>("launch_capturer", {
            profileId: profile.id,
            allowNonLoopback: false,
          });
        }
        if (lastInstance) selectedCapturerId = lastInstance.id;
        await refreshCapturers();
        message = `Launched ${launchGroupProfiles.length} capturers`;
        return;
      }

      const profile_id = launchProfileId || null;
      const instance = await invoke<CapturerInstance>("launch_capturer", {
        profileId: profile_id,
        allowNonLoopback: false,
      });
      selectedCapturerId = instance.id;
      await refreshCapturers();
      message = `Launched #${instance.id}`;
    } catch (error) {
      await refreshCapturers();
      errorMessage = `launch_capturer failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  async function maybeAutoLaunchSelectedOnStartup(): Promise<void> {
    if (startupAutoLaunchAttempted) return;
    startupAutoLaunchAttempted = true;
    if (!appSettings.autoLaunchSelectedOnStartup) return;
    if (!isValidLaunchTarget(launchTargetId, profiles)) return;
    if (liveCapturers().length > 0) return;
    activeTab = "capturers";
    await launchCapturer();
  }

  async function stopCapturer(id: number) {
    if (busy) return;
    busy = true;
    message = `Stopping #${id}...`;
    errorMessage = "";
    try {
      await invoke("stop_capturer", { id });
      await refreshCapturers();
      message = `Stopped #${id}`;
    } catch (error) {
      errorMessage = `stop_capturer failed: ${String(error)}`;
      message = "";
    } finally {
      busy = false;
    }
  }

  function formatUptime(secs: number): string {
    if (!Number.isFinite(secs) || secs < 0) return "-";
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = Math.floor(secs % 60);
    if (h > 0) return `${h}h${m}m`;
    if (m > 0) return `${m}m${s}s`;
    return `${s}s`;
  }

  /// FPS 値 (frames/sec, packets/sec, datagrams/sec のいずれか) を表示用文字列に
  /// 整形する。`null` は「初回観測前 / 取得不能」を意味し `"-"` を返す。
  /// 10 未満は小数 1 桁、それ以上は整数表示にして table の桁ぶれを抑える。
  function formatFps(value: number | null | undefined): string {
    if (value === null || value === undefined || !Number.isFinite(value)) {
      return "-";
    }
    if (value <= 0) return "0";
    if (value < 10) return value.toFixed(1);
    return Math.round(value).toString();
  }

  function stateClass(state: CapturerState): string {
    return `state state-${state}`;
  }

  function capturerStateLabel(state: CapturerState): string {
    if (!state) return "";
    return state.charAt(0).toUpperCase() + state.slice(1);
  }

  /** テレメトリ列: 数値に単位を付ける（ヘッダー側では単位を出さない）。 */
  function formatTelemetryWithUnit(
    value: number | null | undefined,
    unit: string,
  ): string {
    const core = formatFps(value);
    if (core === "-") return "-";
    return `${core} ${unit}`;
  }

  function bestSourceRate(
    fps: CapturerOutputFps | null | undefined,
  ): number | null {
    if (!fps || fps.sources.length === 0) return null;
    const observed = Math.max(
      ...fps.sources.map((source) => source.observedSourceFps ?? 0),
    );
    if (observed > 0) return observed;
    const emitted = Math.max(
      ...fps.sources.map((source) => source.framesPerSec),
    );
    if (emitted > 0) return emitted;
    const raw = Math.max(...fps.sources.map((source) => source.rawPerSec));
    return raw > 0 ? raw : emitted;
  }

  function bestEngineRate(
    fps: CapturerOutputFps | null | undefined,
  ): number | null {
    if (!fps || fps.sources.length === 0) return null;
    const emitted = Math.max(
      ...fps.sources.map((source) => source.framesPerSec),
    );
    return emitted > 0 ? emitted : null;
  }

  function booleanSettingLabel(value: boolean | null | undefined): string {
    if (value === true) return "Enabled";
    if (value === false) return "Disabled";
    return "-";
  }

  function textSettingLabel(value: string | null | undefined): string {
    const trimmed = (value ?? "").trim();
    return trimmed ? trimmed : "-";
  }
</script>

<svelte:head>
  <title>UN Motion Supervisor</title>
</svelte:head>

<main class="shell">
  <header class="topbar">
    <div class="brand">
      <img src="/un-motion-artwork-supervisor.png" alt="" />
      <div>
        <h1>{$_("app.name")}</h1>
        <p>{$_("app.subtitle")}</p>
      </div>
    </div>
    <div class="status-strip" aria-label="Capturer status summary">
      <span><Activity size={14} />{runningCount} running</span>
      <span class:warn={issueCount > 0}>
        <AlertTriangle size={14} />{issueCount} issues
      </span>
      <span>{message}</span>
    </div>
    <div class="header-actions">
      <div class="theme-switch" aria-label="Theme">
        <button
          class:active={appSettings.themeMode === "system"}
          onclick={() => setThemeMode("system")}>{$_("theme.system")}</button
        >
        <button
          class:active={appSettings.themeMode === "light"}
          onclick={() => setThemeMode("light")}
          ><Sun size={15} />{$_("theme.light")}</button
        >
        <button
          class:active={appSettings.themeMode === "dark"}
          onclick={() => setThemeMode("dark")}
          ><Moon size={15} />{$_("theme.dark")}</button
        >
      </div>
    </div>
  </header>

  <div class="workspace">
    <aside class="side-rail" aria-label="Primary navigation">
      <button
        class:active={activeTab === "capturers"}
        onclick={() => (activeTab = "capturers")}
      >
        <Monitor size={17} />{$_("sidebar.capturers")}
      </button>
      <button
        class:active={activeTab === "profiles"}
        onclick={() => (activeTab = "profiles")}
      >
        <FileCog size={17} />{$_("sidebar.profiles")}
      </button>
      <button
        class:active={activeTab === "logs"}
        onclick={() => (activeTab = "logs")}
      >
        <TerminalSquare size={17} />{$_("sidebar.logs")}
      </button>
      <button
        class:active={activeTab === "app"}
        onclick={() => (activeTab = "app")}
      >
        <Settings size={17} />{$_("sidebar.settings")}
      </button>
      <div class="rail-footer">
        <button
          class="danger"
          disabled={busy || runningCapturers().length === 0}
          onclick={() => void stopAllCapturers()}
          title="Stop all running capturers"
        >
          <AlertTriangle size={16} />{$_("app.stop_all")}
        </button>
      </div>
    </aside>

    {#if actionCountdownOverlay}
      <div class="action-countdown-overlay" aria-live="assertive">
        <div class="action-countdown-card">
          <div class="action-countdown-copy">
            <span class="action-countdown-kicker"
              >{actionCountdownOverlay.subtitle}</span
            >
            <h2>{actionCountdownOverlay.title}</h2>
            <div
              class="action-countdown-number"
              class:sampling={actionCountdownOverlay.phase === "sampling"}
            >
              {#if actionCountdownOverlay.remaining != null}
                {actionCountdownOverlay.remaining}
              {:else}
                GO
              {/if}
            </div>
            <div class="action-countdown-tags">
              {#each actionCountdownOverlay.highlights as highlight (highlight)}
                <span>{highlight}</span>
              {/each}
            </div>
            {#each actionCountdownOverlay.body as line (line)}
              <p>{line}</p>
            {/each}
          </div>
          {#if actionCountdownOverlay.imageSrc}
            <img
              class="action-countdown-image"
              src={actionCountdownOverlay.imageSrc}
              alt=""
            />
          {/if}
        </div>
      </div>
    {/if}

    {#if activeTab === "capturers"}
      <section class="view">
        <div class="toolbar">
          <button
            class="primary"
            disabled={busy || profiles.length === 0}
            onclick={launchCapturer}
          >
            <Play size={16} />{$_("capturers.toolbar.launch")}
          </button>
          <div class="launch-target">
            <button
              type="button"
              class="launch-select-button launch-select-text-only"
              disabled={busy || profiles.length === 0}
              title={launchGroupName || launchProfileSummary?.name || ""}
              onclick={() =>
                (capturersProfileMenuOpen = !capturersProfileMenuOpen)}
            >
              {#if profiles.length === 0}
                <strong>{$_("capturers.toolbar.no_profiles")}</strong>
              {:else if launchGroupName}
                <strong>Group: {launchGroupName}</strong>
              {:else if launchProfileSummary}
                <strong>{launchProfileSummary.name}</strong>
              {:else}
                <strong>{$_("capturers.toolbar.pick_profile")}</strong>
              {/if}
              <ChevronDown size={15} />
            </button>
            {#if capturersProfileMenuOpen && profiles.length > 0}
              <div class="launch-menu">
                {#if profileGroups.length > 0}
                  {#each profileGroups as group (group)}
                    <button
                      type="button"
                      class="launch-menu-item-text-only"
                      class:selected={launchTargetId === `group:${group}`}
                      onclick={() => {
                        launchTargetId = `group:${group}`;
                        capturersProfileMenuOpen = false;
                      }}
                    >
                      <span>
                        <strong>Group: {group}</strong>
                        <small
                          >{profiles.filter(
                            (p) => (p.group ?? "").trim() === group,
                          ).length} profiles</small
                        >
                      </span>
                    </button>
                  {/each}
                  <div class="launch-menu-divider"></div>
                {/if}
                {#each profiles as profile (profile.id)}
                  <button
                    type="button"
                    class="launch-menu-item-text-only"
                    class:selected={launchTargetId === profile.id}
                    onclick={() => {
                      launchTargetId = profile.id;
                      capturersProfileMenuOpen = false;
                    }}
                  >
                    <span>
                      <strong>{profile.name}</strong>
                      <small>{profile.note ? profile.note : profile.id}</small>
                    </span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
          <button onclick={() => void refreshCapturers()}>
            <Activity size={16} />{$_("capturers.toolbar.refresh")}
          </button>
          <label class="toggle-field capturer-toolbar-toggle">
            <input type="checkbox" bind:checked={showStoppedCapturers} />
            <span>終了したプロセスも表示</span>
          </label>
          <div class="toolbar-action-group">
            <button
              type="button"
              class="compact-action-button"
              title="Save one frame snapshot from all running capturers"
              disabled={busy ||
                calibratingCapturerId != null ||
                runningCapturers().length === 0}
              onclick={() => void saveAllCapturersUnmfPose(0)}
            >
              1f:ALL
            </button>
            <button
              type="button"
              class="compact-action-button"
              title="Save one second snapshot from all running capturers"
              disabled={busy ||
                calibratingCapturerId != null ||
                runningCapturers().length === 0}
              onclick={() => void saveAllCapturersUnmfPose(1000)}
            >
              1s:ALL
            </button>
            <button
              type="button"
              class="compact-action-button"
              title="Save three second sample from all running capturers"
              disabled={busy ||
                calibratingCapturerId != null ||
                runningCapturers().length === 0}
              onclick={() => void saveAllCapturersUnmfPose(3000)}
            >
              3s:ALL
            </button>
            <button
              type="button"
              class="compact-action-button"
              title="Clear calibration for all running capturers"
              disabled={busy ||
                calibratingCapturerId != null ||
                runningCapturers().length === 0}
              onclick={() => void clearAllCapturerNeutralCalibrations()}
            >
              ↺:ALL
            </button>
            {#each calibrationPoseOptions as pose (pose.kind)}
              <button
                type="button"
                class="compact-action-button"
                title={`${pose.label} calibration for all running capturers`}
                disabled={busy ||
                  calibratingCapturerId != null ||
                  runningCapturers().length === 0}
                onclick={() =>
                  void calibrateAllCapturersNeutral(
                    pose.kind,
                  )}
              >
                {pose.shortLabel}:ALL
              </button>
            {/each}
          </div>
          <span class="toolbar-message">{message}</span>
        </div>

        <div class="split">
          <section class="panel table-panel" aria-label="Capturer processes">
            <table class="process-table capturer-process-table">
              <colgroup>
                <col class="process-col-status" />
                <col class="process-col-profile" />
                <col class="process-col-source" />
                <col class="process-col-perf" />
                <col class="process-col-output" />
              </colgroup>
              <thead>
                <tr>
                  <th>PID / 状態</th>
                  <th>{$_("capturers.columns.profile")}</th>
                  <th title={$_("capturers.columns.tooltip.source_fps")}
                    >Source</th
                  >
                  <th title={$_("capturers.columns.tooltip.engine_fps")}
                    >{$_("capturers.columns.performance")}</th
                  >
                  <th title="UNMF/Z fps / VMC packets per second"
                    >{$_("capturers.columns.output")}</th
                  >
                </tr>
              </thead>
              <tbody>
                {#each capturerTableCapturers as capturer (capturer.id)}
                  {@const fps = runtimeStatuses[capturer.id]?.fps ?? null}
                  {@const bestSrc = bestSourceRate(fps)}
                  {@const bestEngine = bestEngineRate(fps)}
                  {@const capProfileId = capturer.profileId ?? null}
                  {@const capProf =
                    capProfileId != null
                      ? (profiles.find((p) => p.id === capProfileId) ?? null)
                      : null}
                  {@const capProfileName = capProf?.name ?? capProfileId ?? "—"}
                  {@const capEngineLabel = engineTypeListLabel(
                    capProf?.engine ??
                      capProf?.runtimeSelection?.engine ??
                      null,
                  )}
                  <tr
                    class:selected={selectedCapturerId === capturer.id}
                    onclick={() => (selectedCapturerId = capturer.id)}
                  >
                    <td class="process-cell-status">
                      <strong
                        >{capturer.pid
                          ? `PID ${capturer.pid}`
                          : `exit ${capturer.exitCode ?? "-"}`}</strong
                      >
                      <button
                        type="button"
                        class="process-status-log-button"
                        title="このプロセスのログを表示"
                        onclick={(event) => {
                          event.stopPropagation();
                          jumpToCapturerLog(capturer);
                        }}
                      >
                        <span class={stateClass(capturer.state)}
                          >{capturerStateLabel(capturer.state)}</span
                        >
                      </button>
                    </td>
                    <td
                      class="process-cell-profile"
                      title={`${capProfileName} · ${capEngineLabel}`}
                    >
                      <strong>{capProfileName}</strong>
                      <small>{capEngineLabel}</small>
                    </td>
                    <td
                      class="process-cell-source"
                      title={profileSummarySourceLine(capProf)}
                    >
                      <strong>{profileSummarySourceLine(capProf)}</strong>
                      <small>{capEngineLabel}</small>
                    </td>
                    <td class="process-cell-perf">
                      <span
                        >Engine {formatTelemetryWithUnit(bestEngine, "fps")}</span
                      >
                      <small
                        >Input {formatTelemetryWithUnit(
                          bestSrc ?? profileSummaryOutputFps(capProf),
                          "fps",
                        )}</small
                      >
                    </td>
                    <td class="process-cell-output">
                      <strong
                        >{formatTelemetryWithUnit(
                          fps?.zenohFramesPerSec ?? null,
                          "fps",
                        )}</strong
                      >
                      <small
                        >{formatTelemetryWithUnit(
                          fps?.vmcPacketsPerSec ?? null,
                          "pkt/s",
                        )}</small
                      >
                    </td>
                  </tr>
                {:else}
                  <tr>
                    <td colspan="5" class="empty"
                      >{showStoppedCapturers
                        ? "表示できる Capturer プロセスはありません"
                        : $_("capturers.empty.no_running")}</td
                    >
                  </tr>
                {/each}
              </tbody>
            </table>
          </section>

          <aside class="panel details-panel">
            <h2>{$_("capturers.details.title")}</h2>
            {#if selectedCapturer}
              {@const detail = selectedCapturerProfileDetail}
              {@const runtime = detail?.runtime}
              {@const capturerEngine = normalizeEngineType(
                runtime?.engine ?? selectedCapturerProfileSummary?.engine ?? "",
              )}
              {@const telemetry = (
                selectedRuntime?.snapshot as Record<string, unknown> | null
              )?.output_telemetry as
                | {
                    zenoh?: {
                      base_key_expr?: string | null;
                      sent_frames?: number | null;
                      error_count?: number | null;
                      last_error?: string | null;
                    } | null;
                    vmc?: {
                      target_addr?: string | null;
                      sent_datagrams?: number | null;
                      sent_packets?: number | null;
                      error_count?: number | null;
                      last_error?: string | null;
                    } | null;
                    vrc_osc?: {
                      target_addr?: string | null;
                      vrchat_detected?: boolean | null;
                      sent_datagrams?: number | null;
                      sent_packets?: number | null;
                      skipped_frames?: number | null;
                      process_gate_blocked_frames?: number | null;
                      error_count?: number | null;
                      last_error?: string | null;
                    } | null;
                  }
                | undefined}
              {@const diagnosticSnapshot = selectedRuntime?.snapshot as Record<
                string,
                unknown
              > | null}
              <div class="inspector-command-bar">
                <button
                  type="button"
                  disabled={busy ||
                    calibratingCapturerId != null ||
                    selectedCapturer.state !== "running"}
                  onclick={() => void saveCapturerUnmfPose(selectedCapturer, 0)}
                  title="Save one frame snapshot"
                  >1f Snapshot</button
                >
                <button
                  type="button"
                  disabled={busy ||
                    calibratingCapturerId != null ||
                    selectedCapturer.state !== "running"}
                  onclick={() =>
                    void saveCapturerUnmfPose(selectedCapturer, 1000)}
                  title="Save one second snapshot"
                  >1s Snapshot</button
                >
                <button
                  type="button"
                  disabled={busy ||
                    calibratingCapturerId != null ||
                    selectedCapturer.state !== "running"}
                  onclick={() =>
                    void saveCapturerUnmfPose(selectedCapturer, 3000)}
                  title="Save three second sample"
                  >3s Snapshot</button
                >
                <button
                  type="button"
                  disabled={busy || calibratingCapturerId != null}
                  onclick={() => void restartCapturer(selectedCapturer)}
                  title={$_("capturers.toolbar.restart")}
                  ><RefreshCw size={14} />Restart</button
                >
                <button
                  type="button"
                  class="danger"
                  disabled={busy ||
                    calibratingCapturerId != null ||
                    selectedCapturer.state !== "running"}
                  onclick={() => void stopCapturer(selectedCapturer.id)}
                  title={$_("capturers.toolbar.stop")}
                  ><Square size={14} />Stop</button
                >
              </div>
              <div class="capturer-diagnostic-and-calibration">
                <div
                  class="part-diagnostic-panel"
                  aria-label="Motion part diagnostics"
                >
                  <svg viewBox="0 0 220 260" role="img" aria-hidden="true">
                    <line class="diag-spine" x1="110" y1="82" x2="110" y2="160" />
                    <path
                      class={`diag-segment diag-arm ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "R-Arm"))}`}
                      d="M88 88 L55 112 L40 144"
                    />
                    <path
                      class={`diag-segment diag-arm ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "L-Arm"))}`}
                      d="M132 88 L165 112 L180 144"
                    />
                    <path
                      class={`diag-segment diag-leg ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "R-Leg"))}`}
                      d="M98 157 L86 198 L76 228"
                    />
                    <path
                      class={`diag-segment diag-leg ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "L-Leg"))}`}
                      d="M122 157 L134 198 L148 228"
                    />
                    <path
                      class={`diag-foot ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "R-Foot"))}`}
                      d="M76 228 L58 234"
                    />
                    <path
                      class={`diag-foot ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "L-Foot"))}`}
                      d="M148 228 L168 234"
                    />
                    <path
                      class={`diag-torso-shape ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "Torso"))}`}
                      d="M78 84 L142 84 L132 158 L88 158 Z"
                    />
                    <circle
                      class={`diag-head-shape ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "Head"))}`}
                      cx="110"
                      cy="47"
                      r="24"
                    />
                    <path
                      class={`diag-face-shape ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "Face"))}`}
                      d="M99 49 Q110 58 121 49"
                    />
                    <circle
                      class={`diag-hand-shape ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "R-Hand"))}`}
                      cx="37"
                      cy="148"
                      r="9"
                    />
                    <circle
                      class={`diag-hand-shape ${diagnosticClass(diagnosticForPart(diagnosticSnapshot, runtime, "L-Hand"))}`}
                      cx="183"
                      cy="148"
                      r="9"
                    />
                  </svg>
                  <div class="part-diagnostic-list">
                    {#each [["Head", "Head"], ["Face", "Face"], ["Torso", "Torso"], ["R-Arm", "Right arm"], ["L-Arm", "Left arm"], ["R-Hand", "Right hand"], ["L-Hand", "Left hand"], ["R-Leg", "Right leg"], ["L-Leg", "Left leg"], ["R-Foot", "Right foot"], ["L-Foot", "Left foot"]] as label (label[0])}
                      {@const diag = diagnosticForPart(
                        diagnosticSnapshot,
                        runtime,
                        label[0] as string,
                      )}
                      {@const rate = sourceRateForPart(
                        selectedRuntime?.fps ?? null,
                        capturerEngine,
                        label[0] as string,
                      )}
                      <div
                        class={`part-diagnostic-row ${diagnosticClass(diag)}`}
                      >
                        <strong>{label[1]}</strong>
                        <span>
                          {statusLabel(diag.status)}
                          {confidencePercent(diag)}%
                        </span>
                        <small>{rate ?? "—"}</small>
                      </div>
                    {/each}
                  </div>
                </div>
                <div class="capturer-calibration-panel">
                  <section
                    class="capturer-action-group face-pose-model-panel"
                    class:face-pose-model-panel--disabled={!canUseFacePoseModel(
                      runtime,
                      capturerEngine,
                    )}
                  >
                    <div class="capturer-action-group-heading">
                      <h3>Neutral calibration</h3>
                      <span
                        class:status-applied={calibrationPoseFromRuntime(
                          runtime,
                        ) != null}
                        class="calibration-status-pill"
                      >
                        {neutralCalibrationStatusLabel(runtime)}
                      </span>
                    </div>
                    <div class="calibration-pose-list">
                      {#each calibrationPoseOptions as pose (pose.kind)}
                        <button
                          type="button"
                          class="calibration-pose-command"
                          class:calibration-pose-command--active={calibrationPoseFromRuntime(
                            runtime,
                          ) === pose.kind}
                          disabled={busy ||
                            calibratingCapturerId != null ||
                            selectedCapturer.state !== "running"}
                          onclick={() =>
                            void calibrateCapturerNeutral(
                              selectedCapturer,
                              pose.kind,
                            )}
                          title={`${pose.label} calibration`}
                        >
                          <img src={pose.imageSrc} alt="" />
                          <span>
                            <strong>{pose.label}</strong>
                          </span>
                        </button>
                      {/each}
                    </div>
                    <button
                      type="button"
                      class="calibration-clear-button"
                      disabled={busy ||
                        calibratingCapturerId != null ||
                        selectedCapturer.state !== "running" ||
                        calibrationPoseFromRuntime(runtime) == null}
                      onclick={() =>
                        void clearCapturerNeutralCalibration(selectedCapturer)}
                    >
                      Clear
                    </button>
                  </section>
                  <section class="capturer-action-group">
                    <div class="capturer-action-group-heading">
                      <h3>{$_("profiles.editor.face_head_model")}</h3>
                      <span
                        class:status-applied={hasFacePoseModel(runtime)}
                        class:status-unavailable={!canUseFacePoseModel(
                          runtime,
                          capturerEngine,
                        )}
                        class="calibration-status-pill"
                      >
                        {facePoseModelStatusLabel(runtime, capturerEngine)}
                      </span>
                    </div>
                    <label
                      class="toggle-field face-pose-model-toggle"
                      class:face-pose-model-toggle--disabled={!canUseFacePoseModel(
                        runtime,
                        capturerEngine,
                      )}
                      class:face-pose-model-toggle--missing={canUseFacePoseModel(
                        runtime,
                        capturerEngine,
                      ) && !hasFacePoseModel(runtime)}
                    >
                      <input
                        type="checkbox"
                        checked={hasFacePoseModel(runtime)}
                        disabled={busy ||
                          calibratingCapturerId != null ||
                          selectedCapturer.state !== "running" ||
                          !canUseFacePoseModel(runtime, capturerEngine)}
                        onchange={(event) => {
                          const checked = (
                            event.currentTarget as HTMLInputElement
                          ).checked;
                          if (checked) {
                            void buildCapturerFacePoseModel(selectedCapturer);
                          } else {
                            void clearCapturerFacePoseModel(selectedCapturer);
                          }
                        }}
                      />
                      <span>{$_("profiles.editor.face_head_model")}</span>
                    </label>
                    {#if !canUseFacePoseModel(runtime, capturerEngine)}
                      <p class="face-pose-model-help">
                        {$_("profiles.editor.face_head_model_unavailable")}
                      </p>
                    {/if}
                    {#if canUseFacePoseModel(runtime, capturerEngine) && !hasFacePoseModel(runtime)}
                      <div class="compact-warning-pill" role="status">
                        {$_("profiles.editor.face_head_model_warning")}
                      </div>
                    {/if}
                  </section>
                </div>
              </div>
              <dl class="capturer-overview">
                <dt>{$_("capturers.details.profile")}</dt>
                <dd>
                  {detail?.name ??
                    selectedCapturerProfileSummary?.name ??
                    selectedCapturerProfileId ??
                    $_("capturers.details.none")}
                </dd>
                <dt>Path</dt>
                <dd>{detail?.path ?? $_("capturers.details.none")}</dd>
                <dt>{$_("capturers.details.pid")}</dt>
                <dd>{selectedCapturer.pid ?? $_("capturers.details.none")}</dd>
                <dt>{$_("capturers.details.uptime")}</dt>
                <dd>
                  {formatUptime(
                    selectedRuntime?.uptimeSecs ?? selectedCapturer.uptimeSecs,
                  )}
                </dd>
                <dt>Engine Type</dt>
                <dd>{engineTypeListLabel(runtime?.engine ?? null)}</dd>
                <dt>{$_("capturers.details.state")}</dt>
                <dd>
                  <span class={stateClass(selectedCapturer.state)}>
                    {capturerStateLabel(selectedCapturer.state)}
                  </span>
                </dd>
                <dt>{$_("capturers.details.bind")}</dt>
                <dd>
                  {selectedCapturer.bindAddr ?? $_("capturers.details.none")}
                </dd>
                <dt>{$_("capturers.details.healthy")}</dt>
                <dd>
                  {selectedRuntime?.healthy
                    ? $_("capturers.details.yes")
                    : $_("capturers.details.no")}
                </dd>
                {#if selectedRuntime?.note}
                  <dt>{$_("capturers.details.note")}</dt>
                  <dd>{selectedRuntime.note}</dd>
                {/if}
                {#if selectedCapturer.exitCode !== null}
                  <dt>{$_("capturers.details.exit")}</dt>
                  <dd>{selectedCapturer.exitCode}</dd>
                {/if}
                {#if capturerEngine === "mediapipe-native"}
                  <dt>Source</dt>
                  <dd>{textSettingLabel(detail?.pipeline.input)}</dd>
                  <dt>Camera device</dt>
                  <dd>{textSettingLabel(runtime?.device)}</dd>
                  <dt>Camera setting</dt>
                  <dd>
                    {textSettingLabel(runtime?.resolution)}
                    {#if detail?.pipeline.inputFps || detail?.pipeline.inputPixelFormat}
                      <small>
                        requested {detail?.pipeline.inputFps ?? "?"} fps
                        {detail?.pipeline.inputPixelFormat ?? "default-format"}
                      </small>
                    {/if}
                  </dd>
                  <dt>Output FPS</dt>
                  <dd>{runtime?.fps ?? $_("capturers.details.none")}</dd>
                {:else if capturerEngine === "vmc"}
                  <dt>Source</dt>
                  <dd>VMC receive (UDP)</dd>
                  <dt>VMC listen</dt>
                  <dd>{textSettingLabel(runtime?.vmcReceiveListenAddr)}</dd>
                {:else if capturerEngine === "ifacialmocap"}
                  <dt>Source</dt>
                  <dd>iFacialMocap receive (UDP)</dd>
                  <dt>iFacialMocap listen</dt>
                  <dd>
                    {textSettingLabel(runtime?.ifacialmocapReceiveListenAddr)}
                  </dd>
                {/if}
                <dt>UNMF/Z out</dt>
                <dd>{booleanSettingLabel(runtime?.zenohEnabled)}</dd>
                <dt>UNMF/Z key expression</dt>
                <dd>
                  {textSettingLabel(
                    runtime?.zenohKeyExpr ?? telemetry?.zenoh?.base_key_expr,
                  )}
                </dd>
                <dt>UNMF/Z sent</dt>
                <dd>
                  {formatTelemetryWithUnit(
                    selectedRuntime?.fps?.zenohFramesPerSec ?? null,
                    "fps",
                  )}
                  <small>{telemetry?.zenoh?.sent_frames ?? 0} frames</small>
                </dd>
                <dt>VMC out</dt>
                <dd>{booleanSettingLabel(runtime?.vmcEnabled)}</dd>
                <dt>VMC target</dt>
                <dd>
                  {textSettingLabel(
                    runtime?.vmcTargetAddr ?? telemetry?.vmc?.target_addr,
                  )}
                </dd>
                <dt>VMC sent</dt>
                <dd>
                  {formatTelemetryWithUnit(
                    selectedRuntime?.fps?.vmcPacketsPerSec ?? null,
                    "pkt/s",
                  )}
                  <small
                    >{telemetry?.vmc?.sent_datagrams ?? 0} datagrams / {telemetry
                      ?.vmc?.sent_packets ?? 0} packets</small
                  >
                </dd>
                <dt>VRC (VRCFT) / OSC out</dt>
                <dd>{booleanSettingLabel(runtime?.vrcOscEnabled)}</dd>
                <dt>VRC OSC target</dt>
                <dd>
                  {textSettingLabel(
                    runtime?.vrcOscTargetAddr ?? telemetry?.vrc_osc?.target_addr,
                  )}
                </dd>
                <dt>VRChat detected</dt>
                <dd>{booleanSettingLabel(telemetry?.vrc_osc?.vrchat_detected)}</dd>
                <dt>VRC OSC sent</dt>
                <dd>
                  {formatTelemetryWithUnit(
                    selectedRuntime?.fps?.vrcOscPacketsPerSec ?? null,
                    "pkt/s",
                  )}
                  <small
                    >{telemetry?.vrc_osc?.sent_datagrams ?? 0} datagrams / {telemetry
                      ?.vrc_osc?.sent_packets ?? 0} packets · skipped {telemetry
                      ?.vrc_osc?.skipped_frames ?? 0}</small
                  >
                </dd>
                {#if selectedRuntime?.fps?.sources.length}
                  {#each selectedRuntime.fps.sources as src (src.streamId)}
                    <dt>Source ({src.kind})</dt>
                    <dd>
                      {formatTelemetryWithUnit(
                        src.observedSourceFps ??
                          (src.framesPerSec > 0
                            ? src.framesPerSec
                            : src.rawPerSec),
                        "fps",
                      )}
                      <small
                        >raw {formatFps(src.rawPerSec)}/s · stream {src.streamId}
                        · {src.sourceId}</small
                      >
                    </dd>
                  {/each}
                {/if}
                {#if telemetry?.zenoh?.last_error}
                  <dt>Zenoh last error</dt>
                  <dd class="bad">{telemetry.zenoh.last_error}</dd>
                {/if}
                {#if telemetry?.vmc?.last_error}
                  <dt>VMC last error</dt>
                  <dd class="bad">{telemetry.vmc.last_error}</dd>
                {/if}
                {#if telemetry?.vrc_osc?.last_error}
                  <dt>VRC OSC last error</dt>
                  <dd class="bad">{telemetry.vrc_osc.last_error}</dd>
                {/if}
              </dl>
            {:else}
              <p class="empty">{$_("capturers.details.none_selected")}</p>
            {/if}
          </aside>
        </div>
      </section>
    {:else if activeTab === "profiles"}
      <section
        class="view settings-view"
        aria-label="Profiles"
        onpointerover={updateProfileHintFromEvent}
        onfocusin={updateProfileHintFromEvent}
        onpointerleave={clearProfileHint}
      >
        <div class="toolbar">
          <button
            class="primary"
            data-hint="新規作成: 既定の設定で新しいプロファイルを作成します。まずはこのまま起動し、必要な項目だけ調整する使い方を想定しています。"
            disabled={busy}
            onclick={() => void createNewProfile()}
          >
            <Plus size={16} />{$_("profiles.actions.new")}
          </button>
          <button
            data-hint="Quick Launch: 選択中のプロファイルでCapturerを起動します。よく使うプロファイルをすばやく試すための操作です。"
            disabled={busy || !selectedProfileId}
            onclick={() => void quickLaunchSelectedProfile()}
            title={$_("profiles.actions.quick_launch_hint")}
          >
            <Play size={16} />{$_("profiles.actions.quick_launch")}
          </button>
          <button
            data-hint="複製: 選択中のプロファイルをコピーします。少しだけ違う設定を試したい場合に使います。"
            disabled={busy || !selectedProfileId}
            onclick={() => void duplicateSelectedProfile()}
          >
            <Copy size={16} />{$_("profiles.actions.duplicate")}
          </button>
          <button
            class="danger hold-delete"
            data-hint="削除: 選択中のプロファイルを長押しで削除します。誤操作を防ぐため、クリックだけでは削除されません。"
            disabled={busy || !selectedProfileId}
            title={$_("profiles.actions.delete_hold_hint")}
            style={`--hold-progress: ${deleteHoldTargetId === selectedProfileId ? deleteHoldProgress : 0}`}
            onpointerdown={() => startDeleteHold(selectedProfileId)}
            onpointerup={cancelDeleteHold}
            onpointercancel={cancelDeleteHold}
            onpointerleave={cancelDeleteHold}
            onkeydown={(event) => {
              if (event.key === " " || event.key === "Enter") {
                event.preventDefault();
                startDeleteHold(selectedProfileId);
              }
            }}
            onkeyup={cancelDeleteHold}
          >
            <span class="hold-fill"></span><Trash2 size={16} /><span
              >{$_("profiles.actions.delete")}</span
            >
          </button>
          <button
            data-hint="フォルダーを開く: プロファイル設定ファイルが保存されている場所を開きます。手動バックアップや調査に使います。"
            disabled={busy}
            onclick={() => void revealProfilesDir()}
          >
            <FolderOpen size={16} />{$_("profiles.actions.open_folder")}
          </button>
        </div>

        <div class="split settings-split">
          <div class="panel setting-list">
            {#if profiles.length === 0}
              <p class="empty">{$_("profiles.empty_workspace")}</p>
            {:else}
              <div
                class:drag-active={Boolean(draggedProfileId)}
                class="setting-list-items"
              >
                {#each profiles as profile (profile.id)}
                  {@const runningCount = runningCountForProfile(profile.id)}
                  <div
                    class="setting-list-row"
                    animate:flip={{ duration: 120 }}
                  >
                    {#if draggedProfileId === profile.id && profilePointerDrag?.active}
                      <div class="drag-placeholder" aria-hidden="true"></div>
                    {/if}
                    <button
                      data-profile-id={profile.id}
                      data-hint={`${profile.name}: このプロファイルを選択して設定を編集します。左端のハンドルで並び順を変更できます。`}
                      class:selected={selectedProfileId === profile.id}
                      class:dragging={draggedProfileId === profile.id}
                      style:--drag-left={draggedProfileId === profile.id &&
                      profilePointerDrag?.active
                        ? `${profilePointerDrag.currentX - profilePointerDrag.offsetX}px`
                        : null}
                      style:--drag-top={draggedProfileId === profile.id &&
                      profilePointerDrag?.active
                        ? `${profilePointerDrag.currentY - profilePointerDrag.offsetY}px`
                        : null}
                      style:--drag-width={draggedProfileId === profile.id &&
                      profilePointerDrag?.active
                        ? `${profilePointerDrag.width}px`
                        : null}
                      style:--drag-height={draggedProfileId === profile.id &&
                      profilePointerDrag?.active
                        ? `${profilePointerDrag.height}px`
                        : null}
                      draggable="false"
                      disabled={busy}
                      onclick={() => {
                        if (suppressProfileClick) {
                          suppressProfileClick = false;
                          return;
                        }
                        void selectProfile(profile.id);
                      }}
                    >
                      <span
                        class="drag-handle"
                        title="Drag to reorder"
                        aria-label="Drag to reorder"
                        role="button"
                        tabindex="0"
                        onpointerdown={(event) =>
                          beginProfilePointerDrag(event, profile.id)}
                      >
                        <GripVertical size={16} />
                      </span>
                      <img src={iconSrc(profile.iconPath)} alt="" />
                      <span class="setting-card-body">
                        <strong>{profile.name}</strong>
                        <small
                          >{profile.group
                            ? `${profile.group} · `
                            : ""}{profile.note
                            ? profile.note
                            : engineTypeListLabel(
                                profile.engine ??
                                  profile.runtimeSelection?.engine ??
                                  null,
                              )}</small
                        >
                      </span>
                      {#if runningCount > 0}
                        <span class="storage-badge storage-user">
                          {runningCount === 1
                            ? "Running"
                            : `Running x${runningCount}`}
                        </span>
                      {/if}
                    </button>
                  </div>
                {/each}
              </div>
            {/if}
          </div>

          <div class="panel editor-panel">
            {#if profileDetail}
              <div class="section-header-row">
                <h2>{$_("profiles.editor.profile_setting_heading")}</h2>
                <small class="muted-small">{profileDetail.name}</small>
              </div>

              <div
                class="setting-editor"
                role="group"
                aria-label="Profile settings"
              >
                <section
                  class="editor-section profile-section"
                  data-hint="Profile identity: Profiles一覧、Launch対象、Capturer詳細に表示される基本情報です。"
                >
                  <div class="identity-row">
                    <button
                      class="icon-picker"
                      disabled={busy}
                      title={$_("profiles.editor.change_icon")}
                      onclick={() => void browseProfileIcon()}
                    >
                      <img src={iconSrc(profileDetail.iconPath)} alt="" />
                      <span><FolderOpen size={13} /></span>
                    </button>
                    <label>
                      <span>{$_("profiles.editor.name")}</span>
                      <input
                        type="text"
                        value={profileDetail.name}
                        onchange={(event) =>
                          void updateProfileField(
                            "name",
                            (event.currentTarget as HTMLInputElement).value,
                          )}
                      />
                    </label>
                  </div>
                  <label>
                    <span>{$_("profiles.editor.group")}</span>
                    <input
                      type="text"
                      value={profileDetail.group}
                      onchange={(event) =>
                        void updateProfileField(
                          "group",
                          (event.currentTarget as HTMLInputElement).value,
                        )}
                    />
                  </label>
                  <label class="path-field icon-path-field">
                    <span>{$_("profiles.editor.icon")}</span>
                    <input
                      value={profileDetail.iconPath ?? ""}
                      disabled={busy}
                      placeholder={$_("profiles.editor.default_icon")}
                      onchange={(event) =>
                        void updateProfileField(
                          "icon_path",
                          (event.currentTarget as HTMLInputElement).value,
                        )}
                    />
                    <button
                      class="field-button"
                      disabled={busy}
                      onclick={() => void browseProfileIcon()}
                    >
                      <FolderOpen size={15} />{$_("profiles.editor.browse")}
                    </button>
                  </label>
                  <label class="notes-field">
                    <span>{$_("profiles.editor.note")}</span>
                    <textarea
                      value={profileDetail.note}
                      onchange={(event) =>
                        void updateProfileField(
                          "note",
                          (event.currentTarget as HTMLTextAreaElement).value,
                        )}
                    ></textarea>
                  </label>
                </section>

                <!-- ============================================================
                     Engine (top-level): どのエンジンでプロファイルが姿勢推定 / 受信
                     をするかをユーザーに最初に決めさせる。Engine Type に応じて以降の
                     Source / Modifier / Output セクションで露出される設定が決まる。
                     ============================================================ -->
                <section
                  class="editor-section"
                  data-hint="Engine: このプロファイルがMediaPipeで推定するか、外部モーションを受信するかを決めます。"
                >
                  <div class="section-title-row">
                    <h3>{$_("profiles.editor.engine")}</h3>
                    <span class="setting-scope"
                      >{$_("profiles.editor.runtime")}</span
                    >
                  </div>
                  <div class="section-grid">
                    <label
                      data-hint="Engine Type: 入力処理の種類です。Webcamやファイル入力ならMediaPipe Nativeを使います。"
                    >
                      <span>{$_("profiles.editor.engine_type")}</span>
                      <select
                        value={engineType}
                        onchange={(event) =>
                          void updateEngineType(
                            (event.currentTarget as HTMLSelectElement)
                              .value as EngineType,
                          )}
                      >
                        <option value="mediapipe-native"
                          >MediaPipe Native (webcam / file)</option
                        >
                        <option value="vmc">VMC receive (UDP)</option>
                        <option value="ifacialmocap"
                          >iFacialMocap receive (UDP)</option
                        >
                      </select>
                    </label>
                    <label
                      data-hint="Output FPS: 空欄では入力FPSに追従します。明示するとUNMF/ZやVMC送信をそのFPSに制限します。"
                    >
                      <span>{$_("profiles.editor.output_fps")}</span>
                      <select
                        value={profileDetail.runtime.fps ?? ""}
                        onchange={(event) => {
                          const raw = (event.currentTarget as HTMLSelectElement)
                            .value;
                          void updateProfileField(
                            "runtime_selection.fps",
                            raw === "" ? null : Number(raw),
                          );
                        }}
                      >
                        {#each fpsPresets as preset (preset.value ?? "default")}
                          <option value={preset.value ?? ""}
                            >{preset.label}</option
                          >
                        {/each}
                      </select>
                    </label>

                    {#if isMediaPipeEngine}
                      <label
                        data-hint="MediaPipe 実行モード: 通常のWebcam用途ではlive_streamを使います。静止画や動画ファイルの検証時だけ変更します。"
                      >
                        <span
                          >{$_("profiles.editor.mediapipe_running_mode")}</span
                        >
                        <select
                          value={profileDetail.runtime.mediaPipeRunningMode ??
                            ""}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.media_pipe_running_mode",
                              emptyStringToNull(
                                (event.currentTarget as HTMLSelectElement)
                                  .value,
                              ),
                            )}
                        >
                          <option value="">(default: live_stream)</option>
                          <option value="image">image</option>
                          <option value="video">video</option>
                          <option value="live_stream">live_stream</option>
                        </select>
                      </label>
                      <label
                        class="toggle-field"
                        data-hint="MediaPipe Holistic: Body、Hands、Faceをまとめて推定します。通常はONです。OFFにするとMediaPipe由来の姿勢推定は大きく制限されます。"
                      >
                        <input
                          type="checkbox"
                          checked={profileDetail.runtime
                            .mediaPipeHolisticEnabled ?? true}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.media_pipe_holistic_enabled",
                              (event.currentTarget as HTMLInputElement).checked,
                            )}
                        />
                        <span>{$_("profiles.editor.mediapipe_holistic")}</span>
                      </label>
                      <label
                        class="toggle-field"
                        data-hint={$_("profiles.editor.hints.input_denoise")}
                      >
                        <input
                          type="checkbox"
                          checked={(profileDetail.pipeline.inputDenoiseMode ??
                            "off") === "temporal-iir"}
                          onchange={(event) =>
                            void updateProfileField(
                              "pipeline_components.input_denoise_mode",
                              (event.currentTarget as HTMLInputElement).checked
                                ? "temporal-iir"
                                : "off",
                            )}
                        />
                        <span>{$_("profiles.editor.input_denoise_temporal_iir")}</span>
                      </label>
                      {#if (profileDetail.pipeline.inputDenoiseMode ?? "off") === "temporal-iir"}
                        <label
                          data-hint={$_("profiles.editor.hints.input_denoise_cutoff")}
                        >
                          <span>
                            {$_("profiles.editor.input_denoise_temporal_iir_cutoff")}
                            {Math.round(
                              profileDetail.pipeline
                                .inputDenoiseTemporalIirHz ?? 10,
                            )}Hz
                          </span>
                          <input
                            type="range"
                            min="1"
                            max="32"
                            step="1"
                            value={profileDetail.pipeline
                              .inputDenoiseTemporalIirHz ?? 10}
                            oninput={(event) =>
                              void updateProfileField(
                                "pipeline_components.input_denoise_temporal_iir_hz",
                                Number(
                                  (event.currentTarget as HTMLInputElement)
                                    .value,
                                ),
                              )}
                          />
                        </label>
                      {/if}
                      <label
                        data-hint="Delegate: Native MediaPipe の推論 backend です。通常はXNNPACKが高速です。問題があればCPUに切り替えるとよいかもしれません。WindowsビルドではGPU delegateは使用できません。"
                      >
                        <span>{$_("profiles.editor.mediapipe_delegate")}</span>
                        <select
                          value={profileDetail.runtime.mediaPipeDelegate ?? ""}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.media_pipe_delegate",
                              emptyStringToNull(
                                (event.currentTarget as HTMLSelectElement)
                                  .value,
                              ),
                            )}
                        >
                          <option value="">(default: XNNPACK)</option>
                          <option value="xnnpack">XNNPACK</option>
                          <option value="cpu">CPU</option>
                        </select>
                      </label>
                      <label
                        class="toggle-field"
                        data-hint="Holistic FlowLimiter: live_stream の未処理フレーム滞留を抑えます。通常はONで、遅延よりスループットを試す場合だけ調整します。"
                      >
                        <input
                          type="checkbox"
                          checked={profileDetail.runtime
                            .mediaPipeHolisticFlowLimiterEnabled ?? true}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.media_pipe_holistic_flow_limiter_enabled",
                              (event.currentTarget as HTMLInputElement).checked,
                            )}
                        />
                        <span
                          >{$_(
                            "profiles.editor.mediapipe_flow_limiter",
                          )}</span
                        >
                      </label>
                      <label
                        data-hint="Delegate threads: XNNPACK/CPU delegate の推論スレッド数です。高くしすぎると他アプリやゲーム配信の余力を奪います。"
                      >
                        <span
                          >{$_("profiles.editor.mediapipe_num_threads")}</span
                        >
                        <select
                          value={profileDetail.runtime.mediaPipeNumThreads ??
                            ""}
                          onchange={(event) => {
                            const raw = (event.currentTarget as HTMLSelectElement)
                              .value;
                            void updateProfileField(
                              "runtime_selection.media_pipe_num_threads",
                              raw === "" ? null : Number(raw),
                            );
                          }}
                        >
                          {#each mediaPipeThreadPresets as preset (preset.value ?? "default")}
                            <option value={preset.value ?? ""}
                              >{preset.label}</option
                            >
                          {/each}
                        </select>
                      </label>
                      <label
                        data-hint="Max in flight: MediaPipe Holistic に同時投入できるフレーム数です。"
                      >
                        <span
                          >{$_(
                            "profiles.editor.mediapipe_flow_max_in_flight",
                          )}</span
                        >
                        <select
                          disabled={!(
                            profileDetail.runtime
                              .mediaPipeHolisticFlowLimiterEnabled ?? true
                          )}
                          value={profileDetail.runtime
                            .mediaPipeHolisticFlowLimiterMaxInFlight ?? 1}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.media_pipe_holistic_flow_limiter_max_in_flight",
                              Number(
                                (event.currentTarget as HTMLSelectElement)
                                  .value,
                              ),
                            )}
                        >
                          {#each mediaPipeFlowLimiterPresets as preset (preset.value)}
                            <option value={preset.value}>{preset.label}</option>
                          {/each}
                        </select>
                      </label>
                      <label
                        data-hint="Max queue: FlowLimiter 前の待ち行列です。0はキューなし、1は最新寄りの安定設定です。"
                      >
                        <span
                          >{$_(
                            "profiles.editor.mediapipe_flow_max_in_queue",
                          )}</span
                        >
                        <select
                          disabled={!(
                            profileDetail.runtime
                              .mediaPipeHolisticFlowLimiterEnabled ?? true
                          )}
                          value={profileDetail.runtime
                            .mediaPipeHolisticFlowLimiterMaxInQueue ?? 1}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.media_pipe_holistic_flow_limiter_max_in_queue",
                              Number(
                                (event.currentTarget as HTMLSelectElement)
                                  .value,
                              ),
                            )}
                        >
                          <option value={0}>0</option>
                          {#each mediaPipeFlowLimiterPresets as preset (preset.value)}
                            <option value={preset.value}>{preset.label}</option>
                          {/each}
                        </select>
                      </label>
                    {/if}
                  </div>
                </section>

                <!-- ============================================================
                     Source: Engine Type に応じた入力ソース設定。
                     MediaPipe 系 → Webcam / Image / Video の Source Type 選択。
                     VMC / iFacialMocap → 受信 listen address。
                     ============================================================ -->
                <section
                  class="editor-section"
                  data-hint="Source: カメラ、ファイル、外部UDPなど、Engineへ渡す入力元を設定します。"
                >
                  <div class="section-title-row">
                    <h3>{$_("profiles.editor.source")}</h3>
                    <span class="setting-scope">{engineType}</span>
                  </div>
                  {#if isMediaPipeEngine}
                    <div class="section-grid">
                      <label
                        data-hint="Source Type: 入力元を選びます。Windowsで仮想カメラや明示的な解像度/FPS指定を重視する場合はDirectShow、扱いやすさ重視ならMediaFoundationが候補です。"
                      >
                        <span>{$_("profiles.editor.source_type")}</span>
                        <select
                          value={mediaPipeSourceType}
                          onchange={(event) =>
                            void updateMediaPipeSourceType(
                              (event.currentTarget as HTMLSelectElement)
                                .value as MediaPipeSourceType,
                            )}
                        >
                          <option value="webcam-directshow"
                            >Webcam (DirectShow)</option
                          >
                          <option value="webcam-mediafoundation"
                            >Webcam (MediaFoundation)</option
                          >
                          <option value="file-image">Image File</option>
                          <option value="file-video">Video File</option>
                        </select>
                      </label>

                      {#if mediaPipeSourceType === "webcam-directshow" || mediaPipeSourceType === "webcam-mediafoundation"}
                        <label
                          data-hint="Camera device: 使用するカメラを選びます。未指定なら利用可能な先頭デバイスを使います。OBS仮想カメラなどはDirectShow側で見える場合があります。"
                        >
                          <span
                            >{$_(
                              "profiles.editor.camera_device",
                            )}{webcamDevicesLoading
                              ? " (loading...)"
                              : ""}</span
                          >
                          {#if webcamDevices.length > 0}
                            <select
                              value={profileDetail.runtime.device ?? ""}
                              onchange={(event) =>
                                void updateProfileField(
                                  "runtime_selection.device",
                                  emptyStringToNull(
                                    (event.currentTarget as HTMLSelectElement)
                                      .value,
                                  ),
                                )}
                            >
                              <option value="">(auto / first available)</option>
                              {#each webcamDevices as device (device.id)}
                                <option value={device.id}>{device.label}</option
                                >
                              {/each}
                              <!-- 既に profile に保存されている device が列挙結果に
                                  含まれていない場合 (Webcam 抜去 / 名称変更 / 別 backend
                                  保存値の引きずり 等) でも値を残せるよう、未知値の
                                  fallback option を追加。-->
                              {#if profileDetail?.runtime.device && !webcamDevices.some((d) => d.id === profileDetail?.runtime.device)}
                                <option value={profileDetail.runtime.device}
                                  >{profileDetail.runtime.device} (not detected)</option
                                >
                              {/if}
                            </select>
                          {:else}
                            <input
                              type="text"
                              value={profileDetail.runtime.device ?? ""}
                              placeholder={mediaPipeSourceType ===
                              "webcam-directshow"
                                ? "(auto / e.g. dshow0:Logicool BRIO)"
                                : "(auto / e.g. cam0:Integrated Camera)"}
                              onchange={(event) =>
                                void updateProfileField(
                                  "runtime_selection.device",
                                  emptyStringToNull(
                                    (event.currentTarget as HTMLInputElement)
                                      .value,
                                  ),
                                )}
                            />
                          {/if}
                        </label>
                        {#if webcamDevicesError}
                          <p
                            class="section-hint"
                            style="color: var(--accent-warn, #c47a1d);"
                          >
                            Webcam enumeration error: {webcamDevicesError}
                          </p>
                        {/if}
                        {#if webcamFormatsError}
                          <p
                            class="section-hint"
                            style="color: var(--accent-warn, #c47a1d);"
                          >
                            Camera format enumeration error (preset list shown
                            as fallback): {webcamFormatsError}
                          </p>
                        {/if}
                        <label
                          data-hint="Camera setting: 解像度、FPS、PixelFormatの組み合わせです。実機対応形式を選ぶと安定します。"
                        >
                          <span
                            >{$_(
                              "profiles.editor.camera_setting",
                            )}{webcamFormatsLoading
                              ? " (loading...)"
                              : ""}</span
                          >
                          <select
                            value={selectedCameraSettingValue(profileDetail)}
                            onchange={(event) =>
                              void updateCameraSetting(
                                (event.currentTarget as HTMLSelectElement)
                                  .value,
                              )}
                          >
                            <!-- 選択中 Camera device から取得した実際の対応 format。
                                enumeration が空 / 失敗したときだけ resolutionPresets に
                                fallback する。-->
                            {#if webcamFormats.length > 0}
                              {#each webcamFormats as format (webcamFormatValue(format))}
                                <option value={webcamFormatValue(format)}
                                  >{format.label}</option
                                >
                              {/each}
                            {:else}
                              <option value="">(default 640x480)</option>
                              {#each resolutionPresets.filter((p) => p.value !== "") as preset (preset.value)}
                                <option value={preset.value}
                                  >{preset.label}</option
                                >
                              {/each}
                            {/if}
                          </select>
                        </label>
                      {/if}

                      {#if mediaPipeSourceType === "file-image" || mediaPipeSourceType === "file-video"}
                        <label
                          class="full-row path-field"
                          data-hint="File path: 静止画または動画ファイルを入力として使います。姿勢推定やプロファイル設定を再現検証したいときに有用です。"
                        >
                          <span>{$_("profiles.editor.file_path")}</span>
                          <div class="path-field-row">
                            <input
                              type="text"
                              value={profileDetail.pipeline.inputPath ?? ""}
                              placeholder={mediaPipeSourceType === "file-image"
                                ? "C:\\path\\to\\photo.png"
                                : "C:\\path\\to\\clip.mp4"}
                              onchange={(event) => {
                                const path = emptyStringToNull(
                                  (event.currentTarget as HTMLInputElement)
                                    .value,
                                );
                                if (mediaPipeSourceType === "file-video") {
                                  void updateVideoInputPath(path);
                                } else {
                                  void updateProfileField(
                                    "pipeline_components.input_path",
                                    path,
                                  );
                                }
                              }}
                            />
                            <button
                              type="button"
                              class="ghost-button"
                              onclick={() => void browseInputFile()}
                            >
                              <FolderOpen size={15} />{$_(
                                "profiles.editor.browse",
                              )}
                            </button>
                          </div>
                        </label>
                      {/if}

                      <!-- Webcam は Camera setting で resolution/fps/pixel format の
                           組み合わせを一括選択する。Input FPS は file-video のみ表示。 -->
                      {#if mediaPipeSourceType === "file-video" || mediaPipeSourceType === "webcam-directshow" || mediaPipeSourceType === "webcam-mediafoundation"}
                        {#if mediaPipeSourceType === "file-video"}
                          <label
                            data-hint="Input FPS: 動画ファイルから読み取った入力FPSです。ファイル選択後に自動設定されます。"
                          >
                            <span>{$_("profiles.editor.input_fps_auto")}</span>
                            <input
                              type="text"
                              readonly
                              value={profileDetail.pipeline.inputFps != null
                                ? `${profileDetail.pipeline.inputFps}`
                                : "(auto after file selection)"}
                            />
                          </label>
                        {/if}
                      {/if}
                      {#if mediaPipeSourceType === "file-image" || mediaPipeSourceType === "file-video"}
                        <label
                          class="toggle-field"
                          data-hint={mediaPipeSourceType === "file-image"
                            ? "Repeat image: 静止画の推定結果を入力FPSで繰り返し送出します。UNAvatarなど複数subscriberへ安定して配るため通常はONにします。"
                            : "Loop video: 動画ファイルを最後まで再生したあと先頭から繰り返します。長時間の検証や調整に使います。"}
                        >
                          <input
                            type="checkbox"
                            checked={profileDetail.pipeline.inputRepeat ??
                              (mediaPipeSourceType === "file-image")}
                            onchange={(event) =>
                              void updateProfileField(
                                "pipeline_components.input_repeat",
                                (event.currentTarget as HTMLInputElement)
                                  .checked,
                              )}
                          />
                          <span>{mediaPipeSourceType === "file-image"
                            ? $_("profiles.editor.repeat_image")
                            : $_("profiles.editor.loop_video")}</span>
                        </label>
                      {/if}
                    </div>
                  {:else if engineType === "vmc"}
                    <div class="section-grid">
                      <label
                        class="full-row"
                        data-hint="VMC listen: 外部アプリからVMC/UDPで受け取る待受アドレスです。同じPC内なら通常は0.0.0.0:39539で十分です。"
                      >
                        <span>{$_("profiles.editor.vmc_listen")}</span>
                        <input
                          type="text"
                          value={profileDetail.runtime.vmcReceiveListenAddr ??
                            ""}
                          placeholder="0.0.0.0:39539 (VMC standard port)"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.vmc_receive_listen_addr",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                    </div>
                  {:else if engineType === "ifacialmocap"}
                    <div class="section-grid">
                      <label
                        class="full-row"
                        data-hint="iFacialMocap listen: iFacialMocap互換データを受け取る待受アドレスです。送信側のIP/ポート設定と合わせます。"
                      >
                        <span>{$_("profiles.editor.ifacialmocap_listen")}</span>
                        <input
                          type="text"
                          value={profileDetail.runtime
                            .ifacialmocapReceiveListenAddr ?? ""}
                          placeholder="0.0.0.0:49983"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.ifacialmocap_receive_listen_addr",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                    </div>
                  {/if}
                </section>

                <!-- ============================================================
                     Modifier: Engine 非依存の bone-transform level の変換。
                     Smoothing → Mirror → Bone Filter の順で適用される。
                     ============================================================ -->
                <section
                  class="editor-section"
                  data-hint="Modifier: Engineが生成したUNMotionFrameを、出力前に補正・平滑化・フィルタリングします。"
                >
                  <div class="section-title-row">
                    <h3>{$_("profiles.editor.modifier")}</h3>
                    <span class="setting-scope"
                      >{$_("profiles.editor.pre_output")}</span
                    >
                  </div>
                  <div class="modifier-groups">
                    <div class="modifier-group modifier-group--filter">
                      <h4>{$_("profiles.editor.filter_heading")}</h4>
                      <div class="modifier-filter-grid">
                        {#each [{ field: "modifier.head_enabled", label: "Head", value: profileDetail.runtime.modifierHeadEnabled, hint: "Head: 頭部ボーンと視線系の出力を許可します。" }, { field: "modifier.face_enabled", label: "Face", value: profileDetail.runtime.modifierFaceEnabled, hint: "Face: 表情ブレンドシェイプと顔まわりの出力を許可します。" }, { field: "modifier.hands_enabled", label: "Hands", value: profileDetail.runtime.modifierHandsEnabled, hint: "Hands: 手首と指の出力を許可します。" }, { field: "modifier.arms_ik_enabled", label: "Arms", value: profileDetail.runtime.modifierArmsIkEnabled, hint: "Arms: 肩、上腕、前腕の出力を許可します。" }, { field: "modifier.torso_enabled", label: "Torso", value: profileDetail.runtime.modifierTorsoEnabled, hint: "Torso: 胸と胴体の出力を許可します。" }, { field: "modifier.legs_enabled", label: "Legs", value: profileDetail.runtime.modifierLegsEnabled, hint: "Legs: 脚の出力を許可します。初期リリースでは新規プロファイルはOFFです。" }, { field: "modifier.feet_enabled", label: "Feet", value: profileDetail.runtime.modifierFeetEnabled, hint: "Feet: 足首と足先の出力を許可します。Legsと併用する想定です。" }] as toggle (toggle.field)}
                          <label class="toggle-field" data-hint={toggle.hint}>
                            <input
                              type="checkbox"
                              checked={toggle.value ?? false}
                              onchange={(event) =>
                                void updateProfileField(
                                  `runtime_selection.${toggle.field}`,
                                  (event.currentTarget as HTMLInputElement)
                                    .checked,
                                )}
                            />
                            <span>{toggle.label}</span>
                          </label>
                        {/each}
                      </div>
                      <label
                        class="range-field"
                        data-hint="Torso pitch scale: Spine/Chest/UpperChest のローカルX軸回転だけを減衰します。1.0で現状通り、0.0で上半身の前後倒れを抑えます。"
                      >
                        <span>Torso pitch scale</span>
                        <input
                          type="range"
                          min="0"
                          max="1"
                          step="0.01"
                          value={torsoPitchScale(profileDetail.runtime)}
                          oninput={(event) =>
                            void updateProfileField(
                              "runtime_selection.modifier.torso_pitch_scale",
                              numberOrNull(
                                (event.currentTarget as HTMLInputElement)
                                  .value,
                              ),
                            )}
                        />
                        <input
                          type="number"
                          min="0"
                          max="1"
                          step="0.01"
                          value={torsoPitchScale(profileDetail.runtime)}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.modifier.torso_pitch_scale",
                              numberOrNull(
                                (event.currentTarget as HTMLInputElement)
                                  .value,
                              ),
                            )}
                        />
                      </label>
                    </div>
                    <div class="modifier-group modifier-group--smoothing">
                      <h4>{$_("profiles.editor.smoothing")}</h4>
                      <div class="smoothing-methods">
                        <div class="smoothing-method">
                          <div class="smoothing-method-header">
                            <label
                              class="toggle-field smoothing-method-title"
                              data-hint="EMA: 現在値へ一定割合で追従する軽量な平滑化です。細かい揺れは抑えやすい一方、強くすると動きが遅れて見えます。"
                            >
                              <input
                                type="checkbox"
                                checked={smoothingEmaEnabled(
                                  profileDetail.runtime,
                                )}
                                onchange={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.smoothing_ema_enabled",
                                    (event.currentTarget as HTMLInputElement)
                                      .checked,
                                  )}
                              />
                              <span>Exponential Moving Average</span>
                            </label>
                            <div
                              class="recommendation-row"
                              aria-label="EMA recommendations"
                            >
                              <span>おすすめ</span>
                              <button
                                type="button"
                                onclick={() => applyEmaRecommendation(0.7)}
                              >
                                弱
                              </button>
                              <button
                                type="button"
                                onclick={() => applyEmaRecommendation(0.45)}
                              >
                                中
                              </button>
                              <button
                                type="button"
                                onclick={() => applyEmaRecommendation(0.25)}
                              >
                                強
                              </button>
                            </div>
                          </div>
                          <label
                            class="range-field"
                            data-hint="Alpha: EMAの追従速度です。大きいほど反応が速く、小さいほど揺れを強く抑えます。"
                          >
                            <span>Alpha</span>
                            <input
                              type="range"
                              min="0"
                              max="1"
                              step="0.01"
                              value={smoothingEmaAlpha(profileDetail.runtime)}
                              oninput={(event) =>
                                void updateProfileField(
                                  "runtime_selection.modifier.smoothing_ema_alpha",
                                  numberOrNull(
                                    (event.currentTarget as HTMLInputElement)
                                      .value,
                                  ),
                                )}
                            />
                            <input
                              type="number"
                              min="0"
                              max="1"
                              step="0.01"
                              value={smoothingEmaAlpha(profileDetail.runtime)}
                              onchange={(event) =>
                                void updateProfileField(
                                  "runtime_selection.modifier.smoothing_ema_alpha",
                                  numberOrNull(
                                    (event.currentTarget as HTMLInputElement)
                                      .value,
                                  ),
                                )}
                            />
                          </label>
                        </div>
                        <div class="smoothing-method">
                          <div class="smoothing-method-header">
                            <label
                              class="toggle-field smoothing-method-title"
                              data-hint="One Euro: 動きの速さに応じて平滑化量を変えます。静止時の小刻みな揺れを抑えつつ、速い動きには追従しやすい方式です。"
                            >
                              <input
                                type="checkbox"
                                checked={smoothingOneEuroEnabled(
                                  profileDetail.runtime,
                                )}
                                onchange={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.smoothing_one_euro_enabled",
                                    (event.currentTarget as HTMLInputElement)
                                      .checked,
                                  )}
                              />
                              <span>One Euro</span>
                            </label>
                            <div
                              class="recommendation-row"
                              aria-label="One Euro recommendations"
                            >
                              <span>おすすめ</span>
                              <button
                                type="button"
                                onclick={() =>
                                  applyOneEuroRecommendation(1.0, 0.12, 1.0)}
                              >
                                弱
                              </button>
                              <button
                                type="button"
                                onclick={() =>
                                  applyOneEuroRecommendation(0.35, 0.08, 1.0)}
                              >
                                中
                              </button>
                              <button
                                type="button"
                                onclick={() =>
                                  applyOneEuroRecommendation(0.15, 0.04, 0.8)}
                              >
                                強
                              </button>
                            </div>
                          </div>
                          <div class="smoothing-method-params">
                            <label
                              class="toggle-field smoothing-option-toggle"
                              data-hint="Confidence adaptive cutoff: 推定信頼度が低いときに平滑化を強めます。手や顔を見失いやすい環境で揺れを抑えたい場合に有効です。"
                            >
                              <input
                                type="checkbox"
                                checked={profileDetail.runtime
                                  .modifierSmoothingConfidenceAdaptiveCutoff ??
                                  false}
                                onchange={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.smoothing_confidence_adaptive_cutoff",
                                    (event.currentTarget as HTMLInputElement)
                                      .checked,
                                  )}
                              />
                              <span>Confidence adaptive cutoff</span>
                            </label>
                            <label
                              class="range-field"
                              data-hint="Min cutoff: 静止時に近い状態での平滑化の基準です。小さいほど揺れを強く抑えますが、動き出しが重くなります。"
                            >
                              <span>Min cutoff</span>
                              <input
                                type="range"
                                min="0.05"
                                max="5"
                                step="0.05"
                                value={profileDetail.runtime
                                  .modifierAdaptiveMinCutoffHz ?? 0.35}
                                oninput={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.adaptive_min_cutoff_hz",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                              <input
                                type="number"
                                min="0.05"
                                max="5"
                                step="0.05"
                                value={profileDetail.runtime
                                  .modifierAdaptiveMinCutoffHz ?? 0.35}
                                onchange={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.adaptive_min_cutoff_hz",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                            </label>
                            <label
                              class="range-field"
                              data-hint="Beta: 動きが速いときにどれだけ平滑化を弱めるかです。大きいほど素早い動きに追従しやすくなります。"
                            >
                              <span>Beta</span>
                              <input
                                type="range"
                                min="0"
                                max="2"
                                step="0.01"
                                value={profileDetail.runtime
                                  .modifierAdaptiveBeta ?? 0.08}
                                oninput={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.adaptive_beta",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                              <input
                                type="number"
                                min="0"
                                max="2"
                                step="0.01"
                                value={profileDetail.runtime
                                  .modifierAdaptiveBeta ?? 0.08}
                                onchange={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.adaptive_beta",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                            </label>
                            <label
                              class="range-field"
                              data-hint="Derivative cutoff: 速度変化の平滑化です。通常は既定値のまま使い、速い動きで不自然な追従が出る場合だけ調整します。"
                            >
                              <span>Derivative cutoff</span>
                              <input
                                type="range"
                                min="0.1"
                                max="5"
                                step="0.05"
                                value={profileDetail.runtime
                                  .modifierAdaptiveDerivativeCutoffHz ?? 1}
                                oninput={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.adaptive_derivative_cutoff_hz",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                              <input
                                type="number"
                                min="0.1"
                                max="5"
                                step="0.05"
                                value={profileDetail.runtime
                                  .modifierAdaptiveDerivativeCutoffHz ?? 1}
                                onchange={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.adaptive_derivative_cutoff_hz",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                            </label>
                          </div>
                        </div>
                      </div>
                    </div>
                    <div class="modifier-group modifier-group--lost-signal">
                      <h4>シグナルロスト時の挙動</h4>
                      <div
                        class="lost-signal-panel"
                        data-hint="Signal lost: カメラが一時的に部位を見失ったときの扱いです。配信中の急な破綻を避けるため、部位ごとに復帰方法を選べます。"
                      >
                        {#each lostSignalParts as typedPart (typedPart)}
                          {@const partBlend = lostSignalPartRestPoseBlend(
                            profileDetail.runtime,
                            typedPart,
                          )}
                          {@const partHold = lostSignalPartHoldSeconds(
                            profileDetail.runtime,
                            typedPart,
                          )}
                          <div class="lost-signal-part">
                            <div class="lost-signal-part-label">
                              {lostSignalPartLabel(typedPart)}
                            </div>
                            <div class="lost-signal-part-options">
                              <label
                                class="lost-signal-option"
                                data-hint="基本姿勢: 見失った部位をTポーズまたはIポーズ寄りの姿勢へ戻します。長く見失う部位を暴れさせたくない場合に使います。"
                              >
                                <input
                                  type="radio"
                                  name={`lost-signal-${typedPart}`}
                                  value="rest-pose"
                                  checked={lostSignalPartBehavior(
                                    profileDetail.runtime,
                                    typedPart,
                                  ) === "rest-pose"}
                                  onchange={() =>
                                    void updateProfileField(
                                      lostSignalPartField(
                                        typedPart,
                                        "behavior",
                                      ),
                                      "rest-pose",
                                    )}
                                />
                                <span class="lost-signal-label">基本姿勢</span>
                                <div class="lost-signal-control">
                                  <span
                                    >Tポーズ {formatRatio(1 - partBlend)}</span
                                  >
                                  <input
                                    type="range"
                                    min="0"
                                    max="1"
                                    step="0.01"
                                    value={partBlend}
                                    oninput={(event) =>
                                      void updateProfileField(
                                        lostSignalPartField(
                                          typedPart,
                                          "rest_pose_blend",
                                        ),
                                        numberOrNull(
                                          (
                                            event.currentTarget as HTMLInputElement
                                          ).value,
                                        ),
                                      )}
                                  />
                                  <span>Iポーズ {formatRatio(partBlend)}</span>
                                </div>
                              </label>
                              <label
                                class="lost-signal-option"
                                data-hint="ホールド: 見失う直前の姿勢を一定時間維持します。短い遮蔽や一瞬の推定抜けを自然に見せたい場合に向きます。"
                              >
                                <input
                                  type="radio"
                                  name={`lost-signal-${typedPart}`}
                                  value="hold"
                                  checked={lostSignalPartBehavior(
                                    profileDetail.runtime,
                                    typedPart,
                                  ) === "hold"}
                                  onchange={() =>
                                    void updateProfileField(
                                      lostSignalPartField(
                                        typedPart,
                                        "behavior",
                                      ),
                                      "hold",
                                    )}
                                />
                                <span class="lost-signal-label">ホールド</span>
                                <div class="lost-signal-control">
                                  <span>維持時間</span>
                                  <input
                                    type="range"
                                    min="0"
                                    max="30"
                                    step="0.1"
                                    value={partHold}
                                    oninput={(event) =>
                                      void updateProfileField(
                                        lostSignalPartField(
                                          typedPart,
                                          "hold_seconds",
                                        ),
                                        numberOrNull(
                                          (
                                            event.currentTarget as HTMLInputElement
                                          ).value,
                                        ),
                                      )}
                                  />
                                  <input
                                    type="number"
                                    min="0"
                                    max="30"
                                    step="0.1"
                                    value={partHold}
                                    onchange={(event) =>
                                      void updateProfileField(
                                        lostSignalPartField(
                                          typedPart,
                                          "hold_seconds",
                                        ),
                                        numberOrNull(
                                          (
                                            event.currentTarget as HTMLInputElement
                                          ).value,
                                        ),
                                      )}
                                  />
                                </div>
                              </label>
                              <label
                                class="lost-signal-option lost-signal-option--simple"
                                data-hint="送信しない: 見失った部位を出力から外します。受信側の既定姿勢や他の制御に任せたい場合に使います。"
                              >
                                <input
                                  type="radio"
                                  name={`lost-signal-${typedPart}`}
                                  value="drop"
                                  checked={lostSignalPartBehavior(
                                    profileDetail.runtime,
                                    typedPart,
                                  ) === "drop"}
                                  onchange={() =>
                                    void updateProfileField(
                                      lostSignalPartField(
                                        typedPart,
                                        "behavior",
                                      ),
                                      "drop",
                                    )}
                                />
                                <span class="lost-signal-label">送信しない</span
                                >
                              </label>
                            </div>
                          </div>
                        {/each}
                        <div class="lost-signal-part lost-signal-common">
                          <div class="lost-signal-part-label">共通</div>
                          <div class="lost-signal-part-options">
                            <label
                              class="lost-signal-recovery"
                              data-hint="復帰イージング: 見失い状態から再検出へ戻るときの補間時間です。大きいほど復帰は滑らかですが、反応は遅くなります。"
                            >
                              <span>復帰イージング</span>
                              <input
                                type="range"
                                min="0"
                                max="5"
                                step="0.05"
                                value={lostSignalRecoverySeconds(
                                  profileDetail.runtime,
                                )}
                                oninput={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.post_process_rules.lost_signal_recovery_seconds",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                              <input
                                type="number"
                                min="0"
                                max="5"
                                step="0.05"
                                value={lostSignalRecoverySeconds(
                                  profileDetail.runtime,
                                )}
                                onchange={(event) =>
                                  void updateProfileField(
                                    "runtime_selection.modifier.post_process_rules.lost_signal_recovery_seconds",
                                    numberOrNull(
                                      (event.currentTarget as HTMLInputElement)
                                        .value,
                                    ),
                                  )}
                              />
                            </label>
                          </div>
                        </div>
                      </div>
                    </div>
                    <div
                      class="modifier-group modifier-group--post-process"
                      class:modifier-group-disabled={!isMediaPipeEngine}
                    >
                      <h4>{$_("profiles.editor.mediapipe_post_process")}</h4>
                      {#if !isMediaPipeEngine}
                        <small class="muted-small"
                          >{$_("profiles.editor.mediapipe_post_process_unavailable")}</small
                        >
                      {/if}
                      <fieldset
                        class="modifier-fieldset"
                        disabled={!isMediaPipeEngine}
                      >
                      <label
                        class="toggle-field toggle-field--wide"
                        data-hint={$_("profiles.editor.hints.anatomical_constraints")}
                      >
                        <input
                          type="checkbox"
                          checked={profileDetail.runtime
                            .modifierAnatomicalConstraints ?? true}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.modifier.post_process_rules.anatomical_constraints",
                              (event.currentTarget as HTMLInputElement)
                                .checked,
                            )}
                        />
                        <span>解剖学的制約</span>
                      </label>
                      <label
                        class="range-field"
                        data-hint="瞼の開き具合: 通常に目を開いた表情が薄目に見える場合は上げ、開きすぎる場合は下げます。0.5が中立です。"
                      >
                        <span>瞼の開き具合</span>
                        <input
                          type="range"
                          min="0"
                          max="1"
                          step="0.01"
                          value={profileDetail.runtime.modifierEyeOpenBias ??
                            0.5}
                          oninput={(event) =>
                            void updateProfileField(
                              "runtime_selection.modifier.eye_open_bias",
                              numberOrNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                        <input
                          type="number"
                          min="0"
                          max="1"
                          step="0.01"
                          value={profileDetail.runtime.modifierEyeOpenBias ??
                            0.5}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.modifier.eye_open_bias",
                              numberOrNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                      <div class="modifier-filter-grid">
                        {#each [{ field: "head_from_face_matrix", label: $_("profiles.editor.face_tracking_head"), value: profileDetail.runtime.modifierHeadFromFaceMatrix, hint: $_("profiles.editor.hints.face_tracking_head") }, { field: "ease_recovery", label: $_("profiles.editor.recovery_easing"), value: profileDetail.runtime.modifierEaseRecovery, hint: $_("profiles.editor.hints.recovery_easing") }, { field: "limit_rotation_jumps", label: $_("profiles.editor.rotation_jump_limit"), value: profileDetail.runtime.modifierLimitRotationJumps, hint: $_("profiles.editor.hints.rotation_jump_limit") }, { field: "head_source_switch_blend", label: $_("profiles.editor.head_source_blend"), value: profileDetail.runtime.modifierHeadSourceSwitchBlend, hint: $_("profiles.editor.hints.head_source_blend") }] as toggle (toggle.field)}
                          <label class="toggle-field" data-hint={toggle.hint}>
                            <input
                              type="checkbox"
                              checked={toggle.value ?? true}
                              onchange={(event) =>
                                void updateProfileField(
                                  `runtime_selection.modifier.post_process_rules.${toggle.field}`,
                                  (event.currentTarget as HTMLInputElement)
                                    .checked,
                                )}
                            />
                            <span>{toggle.label}</span>
                          </label>
                        {/each}
                      </div>
                      <div
                        class="developer-face-model face-pose-model-panel"
                        class:face-pose-model-panel--disabled={!canUseFacePoseModel(
                          profileDetail.runtime,
                          engineType,
                        )}
                      >
                        <div class="developer-face-model-row">
                          <label
                            class="toggle-field"
                            class:face-pose-model-toggle--disabled={!canUseFacePoseModel(
                              profileDetail.runtime,
                              engineType,
                            )}
                            class:face-model-warning-checkbox={canUseFacePoseModel(
                              profileDetail.runtime,
                              engineType,
                            ) &&
                              profileDetail.runtime
                                .modifierFacePoseModelNeutralNoseDropEyeMouth ==
                                null}
                            data-hint={$_("profiles.editor.hints.face_head_model")}
                          >
                            <input
                              type="checkbox"
                              checked={profileDetail.runtime
                                .modifierFacePoseModelNeutralNoseDropEyeMouth !=
                                null &&
                                (profileDetail.runtime
                                  .modifierFacePoseModelEnabled ??
                                  false)}
                              disabled={!canUseFacePoseModel(
                                profileDetail.runtime,
                                engineType,
                              )}
                              onchange={(event) => {
                                const checked = (
                                  event.currentTarget as HTMLInputElement
                                ).checked;
                                if (
                                  checked &&
                                  profileDetail?.runtime
                                    .modifierFacePoseModelNeutralNoseDropEyeMouth ==
                                    null
                                ) {
                                  void buildSelectedProfileFacePoseModel();
                                } else if (checked) {
                                  void updateProfileField(
                                    "runtime_selection.modifier.face_pose_model.enabled",
                                    true,
                                  );
                                } else {
                                  void updateProfileField(
                                    "runtime_selection.modifier.face_pose_model.enabled",
                                    false,
                                  );
                                }
                              }}
                            />
                            <span>{$_("profiles.editor.face_head_model")}</span>
                          </label>
                          <small>
                            {#if profileDetail.runtime.modifierFacePoseModelNeutralNoseDropEyeMouth != null}
                              neutral {profileDetail.runtime.modifierFacePoseModelNeutralNoseDropEyeMouth?.toFixed(
                                3,
                              )}
                              · {profileDetail.runtime
                                .modifierFacePoseModelSampleCount ?? 0} samples
                            {:else}
                              {$_("profiles.editor.not_created")}
                            {/if}
                          </small>
                        </div>
                        {#if !canUseFacePoseModel(profileDetail.runtime, engineType)}
                          <p class="face-pose-model-help">
                            {$_("profiles.editor.face_head_model_unavailable")}
                          </p>
                        {/if}
                        {#if canUseFacePoseModel(profileDetail.runtime, engineType) && profileDetail.runtime.modifierFacePoseModelNeutralNoseDropEyeMouth == null}
                          <div
                            class="face-model-warning profile-face-model-warning"
                            role="status"
                          >
                            <strong
                              >{$_("profiles.editor.face_head_model_warning")}</strong
                            >
                          </div>
                        {/if}
                      </div>
                      </fieldset>
                    </div>
                    <label
                      class="modifier-group mirror-mode-group"
                      data-hint="Mirror mode: 出力の左右やX軸を反転します。カメラ表示の鏡像設定と出力先の左右が合わない場合だけ変更します。"
                    >
                      <span>{$_("profiles.editor.mirror_mode")}</span>
                      <select
                        value={profileDetail.runtime.modifierMirrorMode ?? ""}
                        onchange={(event) =>
                          void updateProfileField(
                            "runtime_selection.modifier.mirror_mode",
                            emptyStringToNull(
                              (event.currentTarget as HTMLSelectElement).value,
                            ),
                          )}
                      >
                        <option value="">(default: normal)</option>
                        <option value="normal">normal (passthrough)</option>
                        <option value="mirror-output"
                          >mirror-output (flip X axis)</option
                        >
                        <option value="swap-sides"
                          >swap-sides (swap L/R bone names only)</option
                        >
                      </select>
                    </label>
                  </div>
                </section>

                <!-- ============================================================
                     Output: Modifier 適用後のフレームを複数 output worker へ
                     同時送出する。
                     ============================================================ -->
                <section
                  class="editor-section"
                  data-hint="Output: Modifier後のUNMotionFrameをUNMF/Z、VMC/UDP、VRC OSCへ送出します。"
                >
                  <div class="section-title-row">
                    <h3>{$_("profiles.editor.output")}</h3>
                  </div>

                  <div class="subgroup">
                    <label
                      class="output-channel-heading"
                      data-hint={$_("profiles.editor.hints.unmfz_publisher")}
                    >
                      <input
                        type="checkbox"
                        checked={profileDetail.runtime.zenohEnabled ?? false}
                        onchange={(event) =>
                          void updateProfileField(
                            "runtime_selection.zenoh_enabled",
                            (event.currentTarget as HTMLInputElement).checked,
                          )}
                      />
                      <span>{$_("profiles.editor.unmfz_publisher")}</span>
                    </label>
                    <div class="section-grid output-channel-fields">
                      <label
                        data-hint="Key expression: Zenoh上の送信先キーです。UNAvatar側と一致している必要があります。通常はun-motion/frameのままで使います。"
                      >
                        <span>{$_("profiles.editor.key_expression")}</span>
                        <input
                          type="text"
                          value={profileDetail.runtime.zenohKeyExpr ?? ""}
                          placeholder="un-motion/frame"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.zenoh_key_expr",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                      <label
                        data-hint="Topic mode: 複数ストリームを扱うときのキー分け方法です。通常はframe、複数入力を分けたい場合はsourceやstream id単位を選びます。"
                      >
                        <span>{$_("profiles.editor.topic_mode")}</span>
                        <select
                          value={profileDetail.runtime.zenohTopicMode ?? ""}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.zenoh_topic_mode",
                              emptyStringToNull(
                                (event.currentTarget as HTMLSelectElement)
                                  .value,
                              ),
                            )}
                        >
                          <option value="">frame (default)</option>
                          <option value="frame">frame</option>
                          <option value="by-primary-source"
                            >by-primary-source</option
                          >
                          <option value="by-stream-id">by-stream-id</option>
                        </select>
                      </label>
                      <label
                        data-hint="Stream ID: このプロファイルの出力を識別する名前です。未指定ならプロファイルIDを使います。複数Capturer運用で区別したいときに設定します。"
                      >
                        <span>{$_("profiles.editor.stream_id")}</span>
                        <input
                          type="text"
                          value={profileDetail.runtime.zenohStreamId ?? ""}
                          placeholder="(profile id)"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.zenoh_stream_id",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                      <label
                        data-hint="Producer: 送信元アプリ名としてUNMF/Zに載せるラベルです。通常はun-motion-capturerのままで構いません。"
                      >
                        <span>{$_("profiles.editor.producer_label")}</span>
                        <input
                          type="text"
                          value={profileDetail.runtime.zenohProducer ?? ""}
                          placeholder="un-motion-capturer"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.zenoh_producer",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                    </div>
                  </div>

                  <div class="subgroup">
                    <label
                      class="output-channel-heading"
                      data-hint={$_("profiles.editor.hints.vmc_udp_sender")}
                    >
                      <input
                        type="checkbox"
                        checked={profileDetail.runtime.vmcEnabled ?? false}
                        onchange={(event) =>
                          void updateProfileField(
                            "runtime_selection.vmc_enabled",
                            (event.currentTarget as HTMLInputElement).checked,
                          )}
                      />
                      <span>{$_("profiles.editor.vmc_udp_sender")}</span>
                    </label>
                    <div class="section-grid output-channel-fields">
                      <label
                        data-hint="Target address: VMC/UDPの送信先です。同じPCのVMC受信対応アプリへ送る場合は通常127.0.0.1:39539のように指定します。"
                      >
                        <span>{$_("profiles.editor.target_address")}</span>
                        <input
                          type="text"
                          value={profileDetail.runtime.vmcTargetAddr ?? ""}
                          placeholder="127.0.0.1:39539"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.vmc_target_addr",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                    </div>
                  </div>

                  <div class="subgroup">
                    <label
                      class="output-channel-heading"
                      data-hint="VRC (VRCFT) / OSC: VRCFaceTracking互換のFaceパラメータをVRChat OSC Avatar Parametersへ送信します。"
                    >
                      <input
                        type="checkbox"
                        checked={profileDetail.runtime.vrcOscEnabled ?? false}
                        onchange={(event) =>
                          void updateProfileField(
                            "runtime_selection.vrc_osc_enabled",
                            (event.currentTarget as HTMLInputElement).checked,
                          )}
                      />
                      <span>VRC (VRCFT) / OSC</span>
                    </label>
                    <div class="section-grid output-channel-fields">
                      <label
                        data-hint="Target address: VRChat OSC inputの送信先です。通常は同じPCの127.0.0.1:9000です。"
                      >
                        <span>{$_("profiles.editor.target_address")}</span>
                        <input
                          type="text"
                          value={profileDetail.runtime.vrcOscTargetAddr ?? ""}
                          placeholder="127.0.0.1:9000"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.vrc_osc_target_addr",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                      <label
                        data-hint="Parameter prefix: avatar側のVRCFT namespaceに合わせます。一般的なVRCFT avatarではFTを使います。"
                      >
                        <span>Parameter prefix</span>
                        <input
                          type="text"
                          value={profileDetail.runtime.vrcOscParameterPrefix ??
                            ""}
                          placeholder="FT"
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.vrc_osc_parameter_prefix",
                              emptyStringToNull(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                      <label
                        class="checkbox-line"
                        data-hint="VRChat OSCQueryからavatar parameterを取得できる時だけ送信します。確認は下の間隔で行います。"
                      >
                        <input
                          type="checkbox"
                          checked={profileDetail.runtime
                            .vrcOscSendOnlyWhenVrchatRunning ?? true}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.vrc_osc_send_only_when_vrchat_running",
                              (event.currentTarget as HTMLInputElement).checked,
                            )}
                        />
                        <span>Only while VRChat is running</span>
                      </label>
                      <label
                        data-hint="VRChat OSCQueryとavatar parameter確認の間隔です。通常は10秒で十分です。"
                      >
                        <span>OSCQuery poll interval</span>
                        <input
                          type="number"
                          min="1"
                          max="3600"
                          step="1"
                          value={profileDetail.runtime
                            .vrcOscProcessPollIntervalSecs ?? 10}
                          onchange={(event) =>
                            void updateProfileField(
                              "runtime_selection.vrc_osc_process_poll_interval_secs",
                              Number(
                                (event.currentTarget as HTMLInputElement).value,
                              ),
                            )}
                        />
                      </label>
                    </div>
                  </div>
                </section>
              </div>
              <div class="profile-hint-bar" aria-live="polite">
                <span>{profileHint || defaultProfileHint}</span>
              </div>
            {:else}
              <p class="empty">
                {$_("profiles.editor.select_or_create")}
              </p>
            {/if}
          </div>
        </div>
      </section>
    {:else if activeTab === "logs"}
      <section class="view logs-view">
        <h2>{$_("logs.title")}</h2>
        <div class="toolbar logs-toolbar">
          <div
            class="segmented-control logs-layout-switch"
            aria-label="Logs layout"
          >
            <button
              class:active={logsLayout === "per-capturer"}
              onclick={() => (logsLayout = "per-capturer")}
              >{$_("logs.toolbar.per_capturer")}</button
            >
            <button
              class:active={logsLayout === "unified"}
              onclick={() => (logsLayout = "unified")}
              >{$_("logs.toolbar.unified")}</button
            >
          </div>
          {#if logsLayout === "unified"}
            <label class="logs-filter-field logs-filter-field--select">
              <span>{$_("logs.toolbar.capturer_label")}</span>
              <select
                value={String(logsCapturerFilter)}
                onchange={(e) => {
                  const v = (e.currentTarget as HTMLSelectElement).value;
                  logsCapturerFilter = v === "all" ? "all" : Number(v);
                }}
              >
                <option value="all">{$_("logs.toolbar.all_capturers")}</option>
                {#each capturers as cap (cap.id)}
                  <option value={String(cap.id)}>#{cap.id} {cap.name}</option>
                {/each}
              </select>
            </label>
          {/if}
          <div class="logs-filter-group">
            <label class="logs-filter-field logs-filter-search">
              <span>{$_("logs.toolbar.filter")}</span>
              <input
                type="text"
                placeholder={$_("logs.toolbar.filter_placeholder")}
                bind:value={logsTextFilter}
              />
            </label>
            <button
              class="ghost-button logs-filter-clear"
              onclick={() => {
                logsTextFilter = "";
                logsCapturerFilter = "all";
              }}
              disabled={logsTextFilter === "" && logsCapturerFilter === "all"}
              title={$_("logs.toolbar.reset_title")}
            >
              <Trash2 size={14} />
              {$_("logs.toolbar.reset_filters")}
            </button>
          </div>
          <label class="logs-filter-field toggle-field">
            <input type="checkbox" bind:checked={logsAutoscroll} />
            <span>{$_("logs.toolbar.autoscroll")}</span>
          </label>
          <div class="logs-toolbar-actions">
            <button
              class="ghost-button"
              onclick={() => copyLogsToClipboard()}
              disabled={capturers.length === 0}
              title={$_("logs.toolbar.copy_title")}
            >
              <Copy size={14} />
              {logsCopyFlash
                ? $_("logs.toolbar.copied")
                : $_("logs.toolbar.copy_all")}
            </button>
            <button
              class="ghost-button"
              onclick={() => void saveLogsToFile()}
              disabled={capturers.length === 0}
              title={$_("logs.toolbar.save_title")}
            >
              <Download size={14} />
              {$_("logs.toolbar.save_txt")}
            </button>
            <button
              class="ghost-button"
              onclick={() => void revealSupervisorLogsDir()}
              title={$_("logs.toolbar.open_folder_title")}
            >
              <FolderOpen size={14} />
              {$_("logs.toolbar.open_folder")}
            </button>
          </div>
        </div>
        <div class="panel logs-panel">
          {#if capturers.length === 0}
            <p class="empty">{$_("logs.body.empty_no_capturers")}</p>
          {:else if logsLayout === "unified"}
            {@const lines = filteredLogLines()}
            {@const totalLines = capturers.reduce(
              (acc, c) => acc + c.stderrTail.length,
              0,
            )}
            <div class="logs-summary-row">
              <span>
                {$_("logs.body.summary", {
                  values: {
                    capturers: capturers.length,
                    shown: lines.length,
                    total: totalLines,
                  },
                })}
                {#if logsTextFilter || logsCapturerFilter !== "all"}
                  {$_("logs.body.summary_filtered")}
                {/if}
              </span>
              <span class="logs-hint">
                {$_("logs.body.hint_buffer", {
                  values: {
                    env_var: "UN_MOTION_LOG=info,un_motion_runtime=debug",
                  },
                })}
              </span>
            </div>
            <div class="logs-stream" bind:this={logsViewRef}>
              {#if lines.length === 0}
                <p class="empty">{$_("logs.body.no_lines_filter")}</p>
              {:else}
                {#each lines as line, idx (idx)}
                  <div class={`logs-line sev-${lineSeverity(line)}`}>
                    {line}
                  </div>
                {/each}
              {/if}
            </div>
          {:else}
            <div class="logs-cards">
              {#each capturers as cap (cap.id)}
                {@const capLines = filteredLinesForCapturer(cap.id)}
                {@const expanded = isCapturerLogExpanded(cap)}
                <article class={`logs-card state-${cap.state}`}>
                  <header class="logs-card-header">
                    <button
                      class="logs-card-toggle"
                      aria-expanded={expanded}
                      onclick={() => toggleCapturerLogExpanded(cap)}
                    >
                      {expanded ? "▼" : "▶"}
                      <strong>#{cap.id} {cap.name}</strong>
                      <span class={stateClass(cap.state)}
                        >{capturerStateLabel(cap.state)}</span
                      >
                      <span class="logs-card-count">
                        {$_("logs.body.card_count", {
                          values: {
                            shown: capLines.length,
                            total: cap.stderrTail.length,
                          },
                        })}
                      </span>
                    </button>
                    <div class="logs-card-actions">
                      <button
                        class="ghost-button"
                        onclick={async () => {
                          try {
                            await navigator.clipboard.writeText(
                              cap.stderrTail.join("\n"),
                            );
                            logsCopyFlash = true;
                            if (logsCopyFlashTimer)
                              clearTimeout(logsCopyFlashTimer);
                            logsCopyFlashTimer = setTimeout(
                              () => (logsCopyFlash = false),
                              1500,
                            );
                          } catch (error) {
                            errorMessage = `clipboard write failed: ${String(error)}`;
                          }
                        }}
                        title={$_("logs.body.card_copy_title")}
                      >
                        <Copy size={13} />{$_("logs.body.card_copy")}
                      </button>
                    </div>
                  </header>
                  {#if expanded}
                    {#if capLines.length === 0}
                      <p class="empty">
                        {cap.stderrTail.length === 0
                          ? $_("logs.body.no_stderr_yet")
                          : $_("logs.body.no_lines_filter")}
                      </p>
                    {:else}
                      <div class="logs-stream logs-stream-card">
                        {#each capLines as line, idx (idx)}
                          <div class={`logs-line sev-${lineSeverity(line)}`}>
                            {line}
                          </div>
                        {/each}
                      </div>
                    {/if}
                  {/if}
                </article>
              {/each}
            </div>
          {/if}
        </div>
      </section>
    {:else}
      <section
        class="view app-settings-view panel"
        aria-label="Settings"
        onpointerover={updateSettingsHintFromEvent}
        onfocusin={updateSettingsHintFromEvent}
        onpointerleave={clearSettingsHint}
      >
        <div class="settings-scroll">
          <section
            class="settings-card"
            data-hint={$_("settings.hints.app_behavior")}
          >
          <div class="settings-card-heading">
            <h2>{$_("settings.app_behavior.title")}</h2>
          </div>
          <div class="settings-layout-grid">
            <div
              class="setting-row setting-row--wide-control"
              data-hint={$_("settings.hints.theme")}
            >
              <span>{$_("settings.theme.label")}</span>
              <div
                class="segmented-control"
                aria-label={$_("settings.theme.label")}
              >
                <button
                  class:active={appSettings.themeMode === "system"}
                  onclick={() => setThemeMode("system")}
                  >{$_("theme.system")}</button
                >
                <button
                  class:active={appSettings.themeMode === "light"}
                  onclick={() => setThemeMode("light")}
                  ><Sun size={15} />{$_("theme.light")}</button
                >
                <button
                  class:active={appSettings.themeMode === "dark"}
                  onclick={() => setThemeMode("dark")}
                  ><Moon size={15} />{$_("theme.dark")}</button
                >
              </div>
            </div>
            <div class="toggles settings-toggle-grid">
              <label
                data-hint={$_("settings.hints.system_tray")}
                ><input
                  type="checkbox"
                  checked={appSettings.systemTrayEnabled}
                  onchange={(event) =>
                    setAppSetting(
                      "systemTrayEnabled",
                      (event.currentTarget as HTMLInputElement).checked,
                    )}
                />{$_("settings.app_behavior.enable_system_tray")}</label
              >
              <label
                data-hint={$_("settings.hints.minimize_to_tray")}
                ><input
                  type="checkbox"
                  checked={appSettings.minimizeToTray}
                  disabled={!appSettings.systemTrayEnabled}
                  onchange={(event) =>
                    setAppSetting(
                      "minimizeToTray",
                      (event.currentTarget as HTMLInputElement).checked,
                    )}
                />{$_("settings.app_behavior.minimize_to_tray")}</label
              >
              <label
                data-hint={$_("settings.hints.close_to_tray_while_running")}
                ><input
                  type="checkbox"
                  checked={appSettings.closeToTrayWhileRunning}
                  disabled={!appSettings.systemTrayEnabled}
                  onchange={(event) =>
                    setAppSetting(
                      "closeToTrayWhileRunning",
                      (event.currentTarget as HTMLInputElement).checked,
                    )}
                />{$_("settings.app_behavior.close_to_tray_while_running")}</label
              >
              <label
                data-hint={$_("settings.hints.start_minimized_to_tray")}
                ><input
                  type="checkbox"
                  checked={appSettings.startMinimizedToTray}
                  disabled={!appSettings.systemTrayEnabled}
                  onchange={(event) =>
                    setAppSetting(
                      "startMinimizedToTray",
                      (event.currentTarget as HTMLInputElement).checked,
                    )}
                />{$_("settings.app_behavior.start_minimized_to_tray")}</label
              >
              <label
                data-hint={$_("settings.hints.stop_children_on_exit")}
                ><input
                  type="checkbox"
                  checked={appSettings.stopCapturersOnExit}
                  onchange={(event) =>
                    setAppSetting(
                      "stopCapturersOnExit",
                      (event.currentTarget as HTMLInputElement).checked,
                    )}
                />{$_("settings.app_behavior.stop_capturers_on_exit")}</label
              >
              <label
                data-hint={$_("settings.hints.quick_launch_jump")}
                ><input
                  type="checkbox"
                  checked={appSettings.jumpToCapturersOnQuickLaunch}
                  onchange={(event) =>
                    setAppSetting(
                      "jumpToCapturersOnQuickLaunch",
                      (event.currentTarget as HTMLInputElement).checked,
                    )}
                />{$_("settings.app_behavior.quick_launch_jump")}</label
              >
              <label
                data-hint={$_("settings.hints.auto_launch_selected_on_startup")}
                ><input
                  type="checkbox"
                  checked={appSettings.autoLaunchSelectedOnStartup}
                  onchange={(event) =>
                    setAppSetting(
                      "autoLaunchSelectedOnStartup",
                      (event.currentTarget as HTMLInputElement).checked,
                    )}
                />{$_("settings.app_behavior.auto_launch_selected_on_startup")}</label
              >
            </div>
            <div
              class="setting-row"
              data-hint={$_("settings.hints.api_worker_threads")}
            >
              <span>{$_("settings.app_behavior.api_worker_threads")}</span>
              <input
                type="number"
                min="1"
                max={logicalCoreCount}
                step="1"
                value={appSettings.apiWorkerThreads}
                onchange={(event) =>
                  setAppSetting(
                    "apiWorkerThreads",
                    Math.round(
                      clampNumber(
                        Number((event.currentTarget as HTMLInputElement).value),
                        1,
                        logicalCoreCount,
                      ),
                    ),
                  )}
              />
            </div>
          </div>
          </section>

          <section
            class="settings-card"
            data-hint={$_("settings.hints.calibration")}
          >
          <div class="settings-card-heading">
            <h2>{$_("settings.calibration.title")}</h2>
          </div>
          <div class="settings-calibration-grid">
            <div class="setting-row" data-hint={$_("settings.hints.calibration_start_delay")}>
              <span>{$_("settings.calibration.start_delay")}</span>
              <input
                type="number"
                min="0"
                max="30"
                step="1"
                value={appSettings.calibrationStartDelaySeconds}
                onchange={(event) =>
                  setAppSetting(
                    "calibrationStartDelaySeconds",
                    clampNumber(
                      Number((event.currentTarget as HTMLInputElement).value),
                      0,
                      30,
                    ),
                  )}
              />
            </div>
            <div class="setting-row" data-hint={$_("settings.hints.calibration_samples")}>
              <span>{$_("settings.calibration.samples")}</span>
              <input
                type="number"
                min="1"
                max="240"
                step="1"
                value={appSettings.calibrationSampleCount}
                onchange={(event) =>
                  setAppSetting(
                    "calibrationSampleCount",
                    Math.round(
                      clampNumber(
                        Number((event.currentTarget as HTMLInputElement).value),
                        1,
                        240,
                      ),
                    ),
                  )}
              />
            </div>
            <div class="setting-row setting-row--volume" data-hint={$_("settings.hints.calibration_sound_volume")}>
              <span>{$_("settings.calibration.sound_volume")}</span>
              <div class="slider-number-pair">
                <input
                  type="range"
                  min="0"
                  max="1"
                  step="0.01"
                  value={appSettings.calibrationSoundVolume}
                  oninput={(event) =>
                    setAppSetting(
                      "calibrationSoundVolume",
                      clampNumber(
                        Number((event.currentTarget as HTMLInputElement).value),
                        0,
                        1,
                      ),
                    )}
                />
                <input
                  type="number"
                  min="0"
                  max="1"
                  step="0.01"
                  value={appSettings.calibrationSoundVolume}
                  onchange={(event) =>
                    setAppSetting(
                      "calibrationSoundVolume",
                      clampNumber(
                        Number((event.currentTarget as HTMLInputElement).value),
                        0,
                        1,
                      ),
                    )}
                />
              </div>
            </div>
          </div>
          <div class="settings-path-stack">
            <div class="setting-row setting-row--wide-control" data-hint={$_("settings.hints.calibration_countdown_sound")}>
              <span>{$_("settings.calibration.countdown_sound")}</span>
              <div class="path-field-row">
                <input
                  type="text"
                  value={appSettings.calibrationCountdownSoundPath ?? ""}
                  placeholder="default: un-calibration-sound-pun.flac"
                  onchange={(event) =>
                    setAppSetting(
                      "calibrationCountdownSoundPath",
                      emptyStringToNull(
                        (event.currentTarget as HTMLInputElement).value,
                      ),
                    )}
                />
                <button
                  type="button"
                  class="ghost-button"
                  onclick={() => void browseCalibrationSound("countdown")}
                >
                  <FolderOpen size={15} />{$_("profiles.editor.browse")}
                </button>
                <button
                  type="button"
                  class="ghost-button"
                  onclick={() =>
                    void playCalibrationSound(
                      appSettings.calibrationCountdownSoundPath,
                      defaultCalibrationCountdownSound,
                    )}
                >
                  <Play size={15} />Test
                </button>
              </div>
            </div>
            <div class="setting-row setting-row--wide-control" data-hint={$_("settings.hints.calibration_start_sound")}>
              <span>{$_("settings.calibration.start_sound")}</span>
              <div class="path-field-row">
                <input
                  type="text"
                  value={appSettings.calibrationStartSoundPath ?? ""}
                  placeholder="default: un-calibration-sound-pon.flac"
                  onchange={(event) =>
                    setAppSetting(
                      "calibrationStartSoundPath",
                      emptyStringToNull(
                        (event.currentTarget as HTMLInputElement).value,
                      ),
                    )}
                />
                <button
                  type="button"
                  class="ghost-button"
                  onclick={() => void browseCalibrationSound("start")}
                >
                  <FolderOpen size={15} />{$_("profiles.editor.browse")}
                </button>
                <button
                  type="button"
                  class="ghost-button"
                  onclick={() =>
                    void playCalibrationSound(
                      appSettings.calibrationStartSoundPath,
                      defaultCalibrationStartSound,
                    )}
                >
                  <Play size={15} />Test
                </button>
              </div>
            </div>
          </div>
          </section>

          <section
            class="settings-card"
            data-hint={$_("settings.hints.external_tools")}
          >
          <div class="settings-card-heading">
            <h2>{$_("settings.external_tools.title")}</h2>
          </div>
          <div class="setting-row setting-row--wide-control" data-hint={$_("settings.hints.ffmpeg_path")}>
            <span>{$_("settings.external_tools.ffmpeg_path")}</span>
            <div class="path-field-row">
              <input
                type="text"
                value={appSettings.externalToolsFfmpegPath ?? ""}
                placeholder="C:\ffmpeg\bin\ffmpeg.exe"
                onchange={(event) =>
                  setAppSetting(
                    "externalToolsFfmpegPath",
                    emptyStringToNull(
                      (event.currentTarget as HTMLInputElement).value,
                    ),
                  )}
              />
              <button
                type="button"
                class="ghost-button"
                onclick={() => void browseFfmpegExecutable()}
              >
                <FolderOpen size={15} />{$_("profiles.editor.browse")}
              </button>
              <button
                type="button"
                class="ghost-button"
                onclick={() => void openFfmpegHome()}
              >
                <Download size={15} />ffmpeg.org
              </button>
            </div>
          </div>
          </section>

          <section
            class="settings-card settings-card--compact"
            data-hint={$_("settings.hints.advanced")}
          >
          <div class="settings-card-heading">
            <h2>{$_("settings.advanced.title")}</h2>
          </div>
          <div class="toggles settings-toggle-grid">
            <label
              data-hint={$_("settings.hints.snapshot_analysis_extras")}
              ><input
                type="checkbox"
                checked={appSettings.snapshotSaveAnalysisExtras}
                onchange={(event) =>
                  setAppSetting(
                    "snapshotSaveAnalysisExtras",
                    (event.currentTarget as HTMLInputElement).checked,
                  )}
              />{$_("settings.advanced.snapshot_analysis_extras")}</label
            >
          </div>
          </section>

          <section
            class="settings-card settings-card--compact"
            data-hint={$_("settings.hints.language")}
          >
          <div class="settings-card-heading">
            <h2>{$_("settings.language.title")}</h2>
          </div>
          <div class="setting-row setting-row--wide-control">
            <span>{$_("language.label")}</span>
            <select
              value={appSettings.locale}
              onchange={(event) => {
                const next = (event.currentTarget as HTMLSelectElement).value;
                setAppSetting("locale", next);
                if (next === "") {
                  void invoke<string>("i18n_resolve_default_locale").then((tag) =>
                    setUiLocale(tag),
                  );
                } else {
                  setUiLocale(next);
                }
              }}
            >
              <option value="">{$_("language.system_option")}</option>
              {#each availableLocales as tag (tag)}
                <option value={tag}
                  >{tag === "ja-JP"
                    ? "日本語 (ja-JP)"
                    : tag === "en-US"
                      ? "English (en-US)"
                      : tag}</option
                >
              {/each}
            </select>
          </div>
          </section>

          <section
            class="settings-card settings-card--compact"
            data-hint={$_("settings.hints.console_window")}
          >
          <div class="settings-card-heading">
            <h2>{$_("settings.console_window.title")}</h2>
          </div>
          <div class="settings-console-fields">
            <div class="setting-row" data-hint={$_("settings.hints.console_window_x")}>
              <span>{$_("settings.console_window.x_outer")}</span>
              <input
                type="number"
                step="1"
                value={appSettings.consoleWindowX ?? ""}
                placeholder={$_("settings.console_window.placeholder_default")}
                oninput={(event) => {
                  const raw = (event.currentTarget as HTMLInputElement).value;
                  setAppSetting(
                    "consoleWindowX",
                    raw.trim() === "" ? null : Number(raw),
                  );
                }}
              />
            </div>
            <div class="setting-row" data-hint={$_("settings.hints.console_window_y")}>
              <span>{$_("settings.console_window.y_outer")}</span>
              <input
                type="number"
                step="1"
                value={appSettings.consoleWindowY ?? ""}
                placeholder={$_("settings.console_window.placeholder_default")}
                oninput={(event) => {
                  const raw = (event.currentTarget as HTMLInputElement).value;
                  setAppSetting(
                    "consoleWindowY",
                    raw.trim() === "" ? null : Number(raw),
                  );
                }}
              />
            </div>
            <div class="setting-row" data-hint={$_("settings.hints.console_window_width")}>
              <span>{$_("settings.console_window.width_inner")}</span>
              <input
                type="number"
                step="1"
                min="0"
                value={appSettings.consoleWindowWidth ?? ""}
                placeholder="1190"
                oninput={(event) => {
                  const raw = (event.currentTarget as HTMLInputElement).value;
                  setAppSetting(
                    "consoleWindowWidth",
                    raw.trim() === "" ? null : Number(raw),
                  );
                }}
              />
            </div>
            <div class="setting-row" data-hint={$_("settings.hints.console_window_height")}>
              <span>{$_("settings.console_window.height_inner")}</span>
              <input
                type="number"
                step="1"
                min="0"
                value={appSettings.consoleWindowHeight ?? ""}
                placeholder="620"
                oninput={(event) => {
                  const raw = (event.currentTarget as HTMLInputElement).value;
                  setAppSetting(
                    "consoleWindowHeight",
                    raw.trim() === "" ? null : Number(raw),
                  );
                }}
              />
            </div>
          </div>
          </section>

          <section
            class="settings-card settings-card--compact"
            data-hint={$_("settings.hints.about")}
          >
          <div class="settings-card-heading">
            <h2>{$_("settings.about.title")}</h2>
          </div>
          <dl class="about-grid">
            <dt>{$_("settings.about.app")}</dt>
            <dd>{$_("settings.about.app_value")}</dd>
            <dt>{$_("settings.about.version")}</dt>
            <dd>{appVersion || $_("app.version_unknown")}</dd>
            <dt>{$_("settings.about.repository")}</dt>
            <dd>
              <a
                href={$_("settings.about.repository_url")}
                target="_blank"
                rel="noreferrer"
                onclick={(event) => {
                  event.preventDefault();
                  void openExternalLink($_("settings.about.repository_url"));
                }}>{$_("settings.about.repository_label")}</a
              >
            </dd>
            <dt>{$_("settings.about.license")}</dt>
            <dd>{$_("settings.about.license_value")}</dd>
          </dl>
          </section>
        </div>
        <div class="profile-hint-bar settings-hint-bar" aria-live="polite">
          <span>{settingsHint || defaultSettingsHint}</span>
        </div>
      </section>
    {/if}
  </div>

  {#if errorMessage}
    <div class="notification notification-error" role="alert">
      <strong>{$_("errors.label")}</strong>
      <span></span>
      <p>{errorMessage}</p>
    </div>
  {/if}
</main>

<style>
  .bad {
    color: var(--accent-warn, #d97706);
    font-weight: 600;
  }

  .calibration-pose-button {
    font-size: 0.78rem;
    font-weight: 800;
    line-height: 1;
  }

  .capturer-action-break {
    flex-basis: 100%;
    height: 0;
  }

  .toolbar-action-group {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
  }

  .compact-action-button {
    min-height: 28px;
    padding: 0 8px;
    font-size: 0.78rem;
    font-weight: 800;
    line-height: 1;
  }

  .developer-face-model {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 10px;
    margin-top: 12px;
    padding-top: 10px;
    border-top: 1px solid var(--border-subtle, rgba(148, 163, 184, 0.18));
  }

  .developer-face-model.face-pose-model-panel--disabled {
    margin-inline: -4px;
    padding: 10px 8px 8px;
    border: 1px solid color-mix(in srgb, var(--muted) 22%, var(--border));
    border-radius: 8px;
    background:
      linear-gradient(180deg, color-mix(in srgb, #000 10%, transparent), transparent),
      color-mix(in srgb, var(--panel-subtle) 46%, #000 10%);
  }

  .developer-face-model.face-pose-model-panel--disabled .face-pose-model-help {
    color: color-mix(in srgb, var(--muted) 70%, var(--text));
  }

  .developer-face-model-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
  }

  .developer-face-model small {
    color: var(--text-muted, #94a3b8);
    white-space: nowrap;
  }

  .profile-face-model-warning {
    margin: 0;
  }

  @media (max-width: 980px) {
    .settings-calibration-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
