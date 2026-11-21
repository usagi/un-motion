/** 機能の入口と関係。値や保存形式の正本は既存のprofile/runtimeに置く。 */
export type FeatureText = { ja: string; en: string };
export type FeatureDestination = "profiles" | "capturers" | "app" | "logs";
export type FeatureGroup = "input" | "motion" | "output" | "operation";
export type MotionFeature = {
	id: string;
	group: FeatureGroup;
	title: FeatureText;
	description: FeatureText;
	settings: FeatureText;
	destination: FeatureDestination;
	anchor?: string;
	engine?: "mediapipe-native" | "vmc" | "ifacialmocap";
};
const t = (ja: string, en: string): FeatureText => ({ ja, en });
export const featureGroups: { id: FeatureGroup; title: FeatureText; description: FeatureText }[] = [
	{
		id: "input",
		title: t("1. 入力を取り込む", "1. Bring motion in"),
		description: t("カメラ・ファイル・他のアプリ → 推定／受信", "Camera, files or another app → tracking / receiving"),
	},
	{
		id: "motion",
		title: t("2. 動きを整える", "2. Shape the motion"),
		description: t("部位を選ぶ → 補正・姿勢合わせ → 共通のモーション", "Select parts → corrections and calibration → motion frame"),
	},
	{
		id: "output",
		title: t("3. 動きを届ける", "3. Send motion out"),
		description: t("同じ補正後の動きを、それぞれの形式で送信", "Send the processed motion through each enabled output"),
	},
	{
		id: "operation",
		title: t("使う構成と動作を管理", "Manage your setup"),
		description: t(
			"プロフィール・複数同時起動・記録・診断・アプリ設定",
			"Profiles, concurrent capture, samples, diagnostics and preferences"
		),
	},
];
export const motionFeatures: MotionFeature[] = [
	{
		id: "camera",
		group: "input",
		title: t("Webカメラ", "Web camera"),
		description: t("カメラから頭・表情・体・手の動きを取り込みます。", "Track your head, face, body and hands with a camera."),
		settings: t(
			"デバイス / DirectShow・MediaFoundation / 解像度 / FPS / Pixel format",
			"Device / DirectShow・MediaFoundation / Resolution / FPS / Pixel format"
		),
		destination: "profiles",
		anchor: "feature-source",
		engine: "mediapipe-native",
	},
	{
		id: "files",
		group: "input",
		title: t("画像・動画ファイル", "Image and video files"),
		description: t("ファイルを入力にして姿勢を推定します。", "Estimate motion from an image or video file."),
		settings: t(
			"画像 / 動画 / パス / 繰り返し / FFmpeg / リサイズ / デノイズ",
			"Image / Video / Path / Repeat / FFmpeg / Resize / Denoise"
		),
		destination: "profiles",
		anchor: "feature-source",
		engine: "mediapipe-native",
	},
	{
		id: "vmc-input",
		group: "input",
		title: t("VMCを受け取る", "Receive VMC"),
		description: t("別アプリの動きを受信し、補正して送り出します。", "Receive motion from another app, process it and send it on."),
		settings: t("VMC / OSC・UDP / 待受アドレス", "VMC / OSC・UDP / Listen address"),
		destination: "profiles",
		anchor: "feature-source",
		engine: "vmc",
	},
	{
		id: "ifacial",
		group: "input",
		title: t("iFacialMocapを受け取る", "Receive iFacialMocap"),
		description: t("iFacialMocapから表情を取り込みます。", "Receive facial motion from iFacialMocap."),
		settings: t("iFacialMocap / UDP / 待受アドレス", "iFacialMocap / UDP / Listen address"),
		destination: "profiles",
		anchor: "feature-source",
		engine: "ifacialmocap",
	},
	{
		id: "engine",
		group: "input",
		title: t("推定・受信の方式", "Tracking and receiving engine"),
		description: t("入力に合わせて処理方式と性能を調整します。", "Choose the processing method and performance for your input."),
		settings: t(
			"エンジン / 出力FPS / MediaPipe / Holistic / delegate / threads / FlowLimiter / queue",
			"Engine / Output FPS / MediaPipe / Holistic / Delegate / Threads / FlowLimiter / Queue"
		),
		destination: "profiles",
		anchor: "feature-engine",
	},
	{
		id: "parts",
		group: "motion",
		title: t("動かす部位", "Motion parts"),
		description: t("送る部位を選び、体の傾きを調整します。", "Choose the parts to send and adjust torso tilt."),
		settings: t(
			"頭 / 表情 / 手・指 / 腕 / 胴体 / 脚 / 足 / Torso pitch scale",
			"Head / Face / Hands / Arms / Torso / Legs / Feet / Torso pitch scale"
		),
		destination: "profiles",
		anchor: "feature-parts",
	},
	{
		id: "smoothing",
		group: "motion",
		title: t("なめらかさ", "Smoothing"),
		description: t("細かな揺れと動きへの追従を調整します。", "Balance jitter reduction and responsiveness."),
		settings: t(
			"EMA / Alpha / One Euro / Min cutoff / Beta / Derivative cutoff / 信頼度",
			"EMA / Alpha / One Euro / Min cutoff / Beta / Derivative cutoff / Confidence"
		),
		destination: "profiles",
		anchor: "feature-smoothing",
	},
	{
		id: "lost",
		group: "motion",
		title: t("見失ったときの動き", "When tracking is lost"),
		description: t("部位別に待機と復帰の振る舞いを決めます。", "Choose how each part waits and recovers."),
		settings: t(
			"頭 / 手 / 腕 / 基本姿勢 / ホールド / 送信停止 / 維持時間 / 復帰イージング",
			"Head / Hands / Arms / Rest pose / Hold / No output / Hold time / Recovery easing"
		),
		destination: "profiles",
		anchor: "feature-lost",
	},
	{
		id: "correction",
		group: "motion",
		title: t("姿勢と表情の補正", "Pose and expression corrections"),
		description: t("推定結果の不自然な変化を整えます。", "Correct unnatural changes in tracking results."),
		settings: t(
			"解剖学的制約 / 瞼 / 頭の推定 / 回転ジャンプ / ソース切替 / 個人補正",
			"Anatomical constraints / Eyelids / Head estimation / Rotation jumps / Source blending / Face model"
		),
		destination: "profiles",
		anchor: "feature-correction",
		engine: "mediapipe-native",
	},
	{
		id: "mirror",
		group: "motion",
		title: t("左右・鏡像", "Sides and mirroring"),
		description: t("出力先に合わせて左右の扱いを選びます。", "Match output handedness to your destination."),
		settings: t("ミラー / X軸反転 / 左右の部位交換", "Mirror / Flip X / Swap sides"),
		destination: "profiles",
		anchor: "feature-mirror",
	},
	{
		id: "calibration",
		group: "motion",
		title: t("姿勢合わせ・個人補正を実行", "Run calibration and face model capture"),
		description: t("稼働中の対象を選び、基準姿勢を合わせます。", "Choose a running capture and calibrate its neutral pose."),
		settings: t(
			"Uポーズ / T-wrist-front / Iポーズ / 解除 / Face Head個人補正",
			"U pose / T-wrist-front / I pose / Clear / Face head personal model"
		),
		destination: "capturers",
		anchor: "feature-calibration",
	},
	{
		id: "unmf",
		group: "output",
		title: t("UNアプリへ送る", "Send to UN apps"),
		description: t("UNMF/Zで同じPCやLANのアプリへ送ります。", "Publish UNMF/Z to apps on this PC or your LAN."),
		settings: t(
			"UNMF/Z / Zenoh / Publisher / LAN / ポート / topic / stream ID",
			"UNMF/Z / Zenoh / Publisher / LAN / Port / Topic / Stream ID"
		),
		destination: "profiles",
		anchor: "feature-output",
	},
	{
		id: "vmc-output",
		group: "output",
		title: t("VMC対応アプリへ送る", "Send to VMC apps"),
		description: t("補正後のモーションをVMCで送ります。", "Send processed motion through VMC."),
		settings: t("VMC / UDP / Sender / 送信先アドレス", "VMC / UDP / Sender / Target address"),
		destination: "profiles",
		anchor: "feature-output",
	},
	{
		id: "vrchat",
		group: "output",
		title: t("VRChatへ表情を送る", "Send expressions to VRChat"),
		description: t("VRCFT互換の表情パラメーターを送信します。", "Send VRCFT-compatible facial parameters."),
		settings: t(
			"VRCFT / OSC / Parameter prefix / 起動中のみ送信 / OSCQuery",
			"VRCFT / OSC / Parameter prefix / Only while running / OSCQuery"
		),
		destination: "profiles",
		anchor: "feature-output",
	},
	{
		id: "profiles",
		group: "operation",
		title: t("プロフィールを整理", "Organize profiles"),
		description: t("設定を保存した構成を作り、用途ごとに管理します。", "Create saved configurations for each use."),
		settings: t(
			"新規 / 複製 / 削除 / 名前 / グループ / アイコン / メモ / 並べ替え",
			"Create / Duplicate / Delete / Name / Group / Icon / Note / Reorder"
		),
		destination: "profiles",
		anchor: "feature-identity",
	},
	{
		id: "sessions",
		group: "operation",
		title: t("同時起動と個別操作", "Concurrent capture and individual controls"),
		description: t("複数の構成を同時に使い、対象別に操作します。", "Use multiple configurations at once and control each capture."),
		settings: t(
			"個別起動 / グループ起動 / 停止 / 全停止 / 再起動 / 状態 / 性能",
			"Launch / Launch group / Stop / Stop all / Restart / Status / Performance"
		),
		destination: "capturers",
	},
	{
		id: "samples",
		group: "operation",
		title: t("モーションのサンプルを保存", "Save motion samples"),
		description: t("稼働中の動きを記録します。カメラ映像の録画ではありません。", "Save running motion samples, not camera footage."),
		settings: t(
			"1フレーム / 1秒 / 3秒 / Snapshot / UNMotionFrame / JSONL",
			"One frame / One second / Three seconds / Snapshot / UNMotionFrame / JSONL"
		),
		destination: "capturers",
	},
	{
		id: "logs",
		group: "operation",
		title: t("ログ・動作の診断", "Logs and diagnostics"),
		description: t("対象ごとの状態と問題を調べます。", "Inspect status and problems for each capture."),
		settings: t("個別 / 統合 / 絞り込み / 自動スクロール / コピー / 保存", "Per capture / Unified / Filter / Autoscroll / Copy / Save"),
		destination: "logs",
	},
	{
		id: "preferences",
		group: "operation",
		title: t("表示・起動・通知", "Appearance, startup and notifications"),
		description: t("アプリ全体の動作と表示を設定します。", "Configure app-wide behavior and appearance."),
		settings: t(
			"配色 / 言語 / トレイ / 自動起動 / 終了時動作 / ウィンドウ / 校正待ち時間・音 / FFmpeg / 分析用情報",
			"Theme / Language / Tray / Auto launch / Exit behavior / Window / Calibration timing and sound / FFmpeg / Analysis extras"
		),
		destination: "app",
	},
];
export function featureText(text: FeatureText, language: string | null | undefined): string {
	return language?.startsWith("ja") ? text.ja : text.en;
}
export function matchingFeatures(query: string): MotionFeature[] {
	const terms = query.normalize("NFKC").toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
	return motionFeatures.filter((feature) => {
		const haystack = [
			feature.id,
			...Object.values(feature.title),
			...Object.values(feature.description),
			...Object.values(feature.settings),
		]
			.join(" ")
			.normalize("NFKC")
			.toLocaleLowerCase();
		return terms.every((term) => haystack.includes(term));
	});
}

/** 別方式の項目を選んでも方式は変更せず、選択欄へ案内する。 */
export function featureAnchor(feature: MotionFeature, engine: string): string | undefined {
	return feature.engine && feature.engine !== engine ? "feature-engine" : feature.anchor;
}
