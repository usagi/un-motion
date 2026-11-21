<script lang="ts">
	import { locale } from "svelte-i18n";
	import { featureText } from "./featureCatalog";
	type Instance = { id: number; name: string; profileId: string | null; state: string };
	let {
		profiles,
		capturers,
		busy,
		onEdit,
		onInspect,
		onLaunch,
		onStop,
		onMap,
	}: {
		profiles: { id: string; name: string; group?: string | null }[];
		capturers: Instance[];
		busy: boolean;
		onEdit: (id: string) => void;
		onInspect: (id: number) => void;
		onLaunch: (id: string) => void;
		onStop: (id: number) => void;
		onMap: () => void;
	} = $props();
	const text = (ja: string, en: string) => featureText({ ja, en }, $locale);
	const live = (s: string) => ["starting", "running", "stopping"].includes(s);
	const instances = (id: string) => capturers.filter((c) => c.profileId === id);
	const orphans = $derived(capturers.filter((c) => !profiles.some((p) => p.id === c.profileId)));
	const count = $derived(capturers.filter((c) => live(c.state)).length);
	const label = (s: string) =>
		({
			starting: text("起動中", "Starting"),
			running: text("実行中", "Running"),
			stopping: text("停止中", "Stopping"),
			exited: text("終了", "Exited"),
			crashed: text("異常終了", "Crashed"),
		})[s] ?? s;
</script>

<section class="motion-home">
	<header>
		<div>
			<p class="eyebrow">UN MOTION</p>
			<h2>{text("ワークスペース", "Workspace")}</h2>
			<p>{text("保存した構成と、いま動いている個体を見渡す。", "Your saved setups and their running instances.")}</p>
		</div>
		<button onclick={onMap}>{text("機能マップを開く", "Open feature map")}</button>
	</header>
	<p class="summary">
		{profiles.length}
		{text("プロファイル", "profiles")} · {count}
		{text("個体が起動・実行・停止処理中", "instances starting, running or stopping")}
	</p>
	<div class="cards">
		{#each profiles as profile (profile.id)}
			<article>
				<div class="identity">
					<span>{profile.group || text("グループなし", "No group")}</span>
					<h3>{profile.name}</h3>
					<p class="profile-id">ID: {profile.id}</p>
				</div>
				<div class="actions">
					<button disabled={busy} onclick={() => onEdit(profile.id)}>{text("設定を開く", "Open settings")}</button><button
						disabled={busy || instances(profile.id).some((c) => live(c.state))}
						onclick={() => onLaunch(profile.id)}>{text("起動", "Start")}</button
					>
				</div>
				<div class="instances">
					<h4>{text("実行個体", "Instances")}</h4>
					{#each instances(profile.id) as instance (instance.id)}
						<div class="instance">
							<button class="inspect" onclick={() => onInspect(instance.id)}>#{instance.id} · {label(instance.state)}</button
							>{#if ["starting", "running"].includes(instance.state)}<button
									disabled={busy}
									onclick={() => onStop(instance.id)}
									aria-label={`${profile.name} #${instance.id} ${text("を停止", "stop")}`}>{text("停止", "Stop")}</button
								>{/if}
						</div>
					{:else}<p>{text("このプロファイルの実行個体はない", "No instances for this profile")}</p>{/each}
				</div>
			</article>
		{:else}<p>{text("プロファイル画面から構成を作成できる。", "Create a setup from Profiles.")}</p>{/each}
	</div>
	{#if orphans.length}<section class="unassigned">
			<h3>{text("保存プロファイルと紐づかない個体", "Instances without a saved profile")}</h3>
			{#each orphans as instance (instance.id)}<div class="instance">
					<button onclick={() => onInspect(instance.id)}>{instance.name} · #{instance.id} · {label(instance.state)}</button
					>{#if ["starting", "running"].includes(instance.state)}<button disabled={busy} onclick={() => onStop(instance.id)}
							>{text("停止", "Stop")} #{instance.id}</button
						>{/if}
				</div>{/each}
		</section>{/if}
	<p class="note">
		{text(
			"実行中はプロセスの状態。入力受信や送信の確認は、個体を開いて診断できる。",
			"Running describes the process. Open an instance to inspect input and output diagnostics."
		)}
	</p>
</section>

<style>
	.motion-home {
		width: 100%;
		box-sizing: border-box;
		display: grid;
		gap: 20px;
		padding: 24px;
		align-content: start;
		max-width: none;
		margin: 0;
	}
	header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 20px;
	}
	h2 {
		font-size: 28px;
		margin: 4px 0 12px;
	}
	p {
		color: var(--muted);
	}
	.eyebrow {
		color: var(--accent);
		font-size: 12px;
		letter-spacing: 0.14em;
	}
	.summary {
		padding: 14px 18px;
		background: var(--panel-subtle);
		border-radius: 12px;
		margin: 0;
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
		gap: 18px;
	}
	article {
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 18px;
		overflow: hidden;
	}
	.identity {
		padding: 20px 20px 8px;
	}
	.identity span {
		color: var(--muted);
		font-size: 12px;
	}
	.profile-id {
		margin: 4px 0 0;
		font-size: 12px;
		overflow-wrap: anywhere;
	}
	h3 {
		margin: 8px 0;
		font-size: 20px;
	}
	.actions {
		display: flex;
		gap: 10px;
		padding: 12px 20px 20px;
	}
	.instances {
		border-top: 1px solid var(--border);
		padding: 16px 20px;
	}
	h4 {
		font-size: 12px;
		color: var(--muted);
		margin: 0 0 12px;
	}
	.instance {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 12px;
		margin: 8px 0;
	}
	.inspect {
		text-align: left;
		flex: 1;
	}
	button {
		background: var(--panel-subtle);
		color: var(--text);
		border: 1px solid var(--border);
		border-radius: 9px;
		padding: 9px 12px;
		cursor: pointer;
	}
	button:hover:not(:disabled) {
		border-color: var(--accent);
	}
	button:disabled {
		opacity: 0.45;
		cursor: default;
	}
	.note {
		font-size: 13px;
	}
	.unassigned {
		border: 1px solid var(--border);
		border-radius: 12px;
		padding: 18px;
	}
	@media (max-width: 700px) {
		header {
			align-items: flex-start;
			flex-direction: column;
		}
		.motion-home {
			padding: 16px;
		}
	}
</style>
