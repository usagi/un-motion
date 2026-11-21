export type ProfileSyncReport = {
	targets: { capturerId: number; accepted: boolean; error: string | null; statusCode?: number | null }[];
	discoveryError: string | null;
};

// Clear only the warning whose previously failed recipients now acknowledge.
// Missing reports/recipients are not evidence of successful delivery.
export class ProfileSyncFeedback {
	private warnings = new Map<string, { text: string; targets: ProfileSyncReport['targets'] }>();
	private onChange: (warnings: string[]) => void;
	constructor(onChange: (warnings: string[]) => void = () => {}) {
		this.onChange = onChange;
	}
	update(profile: { id: string; name: string; syncReport?: ProfileSyncReport }, language: string, currentError: string): string {
		const previous = this.warnings.get(profile.id);
		const report = profile.syncReport;
		if (!report) return currentError;
		// Remove only our previous messages, then render every still-unconfirmed profile.
		// A successful write to one profile must not hide another profile's failure.
		let unrelated = currentError;
		for (const entry of this.warnings.values()) unrelated = unrelated.split(entry.text).join("");
		unrelated = unrelated.trim();
		const pending = new Map(previous?.targets.map(target => [target.capturerId, target]) ?? []);
		for (const target of report.targets) {
			if (target.accepted) pending.delete(target.capturerId);
			else pending.set(target.capturerId, target);
		}
		const targets = [...pending.values()];
		const warning = profileSyncWarning({ ...profile, syncReport: { ...report, targets } }, language);
		if (warning) {
			this.warnings.set(profile.id, { text: warning, targets });
		} else if (previous && !report.discoveryError && !targets.length) {
			this.warnings.delete(profile.id);
		}
		const messages = [...this.warnings.values()].map(entry => entry.text);
		this.onChange(messages);
		return mergeProfileSyncWarnings(unrelated, messages);
	}
}

export function savedProfileRefreshWarning(profileId: string, error: unknown, language: string): string {
	return language.startsWith("ja")
		? `「${profileId}」の設定は保存済みです。その後の画面・状態更新に失敗しました。再保存は不要です。 ${String(error)}`
		: `Settings for ${profileId} were saved, but refreshing the display or status failed. No resave is needed. ${String(error)}`;
}

export function profileSyncWarning(profile: { id: string; name: string; syncReport?: ProfileSyncReport }, language: string): string | null {
	const report = profile.syncReport;
	if (!report) return null; // Older backends do not provide delivery evidence.
	const failures = report.targets.filter(target => !target.accepted);
	if (!report.discoveryError && !failures.length) return null;
	const target = `${profile.name} (${profile.id})`;
	const details = failures.map(item => {
		const reason = item.statusCode === 409
			? language.startsWith("ja")
				? "適用対象が一致しないため反映を拒否しました。稼働中のプロファイルを確認してください。"
				: "Application was rejected because the target does not match. Check the running profile."
			: item.error ?? "unconfirmed";
		return `Capturer #${item.capturerId}: ${reason}`;
	});
	if (report.discoveryError) details.unshift(report.discoveryError);
	return language.startsWith("ja")
		? `「${target}」の設定は保存済みです。次のCapturerへの受け渡しを確認できません。稼働状態を確認してください。 ${details.join(" / ")}`
		: `Settings for ${target} were saved. Delivery to the following Capturers is unconfirmed. Check their runtime status. ${details.join(" / ")}`;
}

// The ordinary operation banner may be cleared independently of delivery evidence.
export function mergeProfileSyncWarnings(message: string, warnings: readonly string[]): string {
	return [message, ...warnings.filter(warning => !message.includes(warning))].filter(Boolean).join("\n");
}
