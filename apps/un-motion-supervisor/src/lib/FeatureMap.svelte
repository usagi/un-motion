<script lang="ts">
	import { locale } from "svelte-i18n";
	import { featureGroups, featureText, matchingFeatures, type MotionFeature } from "./featureCatalog";
	let {
		profiles,
		selectedProfileId,
		engine,
		query = $bindable(""),
		onSelectProfile,
		onOpen,
	}: {
		profiles: { id: string; name: string }[];
		selectedProfileId: string | null;
		engine: string;
		query?: string;
		onSelectProfile: (id: string) => void;
		onOpen: (feature: MotionFeature) => void;
	} = $props();
	const text = (ja: string, en: string) => featureText({ ja, en }, $locale);
	const matches = $derived(matchingFeatures(query));
</script>

<section class="feature-map" aria-labelledby="feature-map-title">
	<header>
		<div>
			<h2 id="feature-map-title">{text("機能マップ", "Feature map")}</h2>
			<p>
				{text(
					"できることと、そのつながりから操作場所を選べます。",
					"See what Motion can do, how it connects, and where to control it."
				)}
			</p>
		</div>
		<label class="search"
			>{text("機能・設定を探す", "Find a feature or setting")}<input
				type="search"
				bind:value={query}
				placeholder={text("例：手、Beta、LAN、トレイ", "Hands, Beta, LAN, tray…")}
			/></label
		>
	</header>
	<div class="target">
		<label for="map-profile">{text("設定を開く対象", "Profile to configure")}</label>
		<select
			id="map-profile"
			value={selectedProfileId ?? ""}
			disabled={!profiles.length}
			onchange={(event) => onSelectProfile(event.currentTarget.value)}
		>
			{#if !profiles.length}<option value="">{text("プロフィール未作成", "No profiles yet")}</option>{/if}
			{#each profiles as profile (profile.id)}<option value={profile.id}>{profile.name} · {profile.id}</option>{/each}
		</select>
		<span>{text("対象を選んでも、稼働中の処理は停止しません。", "Selecting a profile does not stop running captures.")}</span>
	</div>
	{#if query}<p class="result-count" role="status">{text(`${matches.length}件の機能`, `${matches.length} features`)}</p>{/if}
	<div class="map-grid">
		{#each featureGroups as group (group.id)}
			{@const entries = matches.filter((feature) => feature.group === group.id)}
			{#if entries.length}
				<section class="map-group" class:operations={group.id === "operation"} aria-labelledby={`map-${group.id}`}>
					<h3 id={`map-${group.id}`}>{featureText(group.title, $locale)}</h3>
					<p>{featureText(group.description, $locale)}</p>
					{#each entries as feature (feature.id)}
						<button class="feature" data-map-feature={feature.id} onclick={() => onOpen(feature)}>
							<strong>{featureText(feature.title, $locale)} <span aria-hidden="true">↗</span></strong>
							<span>{featureText(feature.description, $locale)}</span>
							<small class="map-search-terms">{featureText(feature.settings, $locale)}</small>
							{#if feature.engine && feature.engine !== engine}<em
									>{text(
										"別の入力方式の機能です。開くと方式の選択へ移動します。",
										"Uses a different input engine. Opens the engine selector."
									)}</em
								>{/if}
						</button>
					{/each}
				</section>
			{/if}
		{/each}
	</div>
	{#if !matches.length}<p>
			{text(
				"一致する機能がありません。短い言葉や方式名でも探せます。",
				"No matching features. Try a shorter term or an algorithm name."
			)}
		</p>{/if}
</section>

<style>
	.feature-map {
		flex: 1;
		min-width: 0;
		min-height: 0;
		overflow: auto;
		padding: 20px;
		color: var(--text);
	}
	header {
		display: flex;
		justify-content: space-between;
		gap: 20px;
		align-items: end;
		flex-wrap: wrap;
	}
	h2 {
		margin: 0;
		font-size: 1.4rem;
	}
	p {
		color: var(--muted);
		font-size: 0.88rem;
		margin: 8px 0 16px;
	}
	.search {
		display: grid;
		gap: 6px;
		min-width: 230px;
		font-size: 0.85rem;
	}
	input,
	select {
		color: var(--text);
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 7px;
		padding: 9px 12px;
		font: inherit;
		max-width: 100%;
	}
	.target {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
		padding: 14px 0;
		margin-bottom: 12px;
		border-bottom: 1px solid var(--border);
	}
	.target label {
		font-weight: 600;
		font-size: 0.85rem;
	}
	.target span {
		color: var(--muted);
		font-size: 0.8rem;
	}
	.map-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 18px;
	}
	.map-group {
		min-width: 0;
	}
	.map-group.operations {
		grid-column: 1 / -1;
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 10px;
	}
	.map-group.operations h3,
	.map-group.operations > p {
		grid-column: 1 / -1;
		margin: 0;
	}
	h3 {
		font-size: 1rem;
		margin: 0;
		color: var(--text);
	}
	.feature {
		display: grid;
		gap: 8px;
		text-align: left;
		width: 100%;
		margin: 10px 0 0;
		padding: 15px;
		background: var(--panel);
		border: 1px solid var(--border);
		border-radius: 10px;
		color: var(--text);
		cursor: pointer;
		white-space: normal;
	}
	.feature:hover {
		border-color: var(--accent);
	}
	.feature strong {
		display: flex;
		justify-content: space-between;
		gap: 8px;
	}
	.feature strong span {
		color: var(--accent);
	}
	.feature > span {
		font-size: 0.82rem;
		color: var(--muted);
	}
	.feature small {
		font-size: 0.78rem;
		line-height: 1.6;
		overflow-wrap: anywhere;
	}
	.feature em {
		font-size: 0.76rem;
		font-style: normal;
		color: var(--muted);
		border-top: 1px solid var(--border);
		padding-top: 8px;
	}
	:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 3px;
	}
	@media (max-width: 1100px) {
		.map-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
		.map-group.operations {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
	@media (max-width: 720px) {
		.map-grid,
		.map-group.operations {
			display: block;
		}
		.map-group {
			margin-bottom: 24px;
		}
	}
</style>
