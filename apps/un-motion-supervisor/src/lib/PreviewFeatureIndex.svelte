<script lang="ts">
 import { featureGroups, motionFeatures, featureText, type MotionFeature } from './featureCatalog';
 export let language = 'ja-JP';
 export let compact = false;
 export let targetName = '';
 export let launchTargetName = '';
 export let savedDefaultName = '';
 export let runtimeTargetName = '';
 export let matchingRuntimeCount = 0;
 export let onOpen: (feature: MotionFeature) => void | Promise<void>;
 function scope(feature: MotionFeature) {
  const ja = language.startsWith('ja');
  if (feature.destination === 'profiles') return `${ja ? '保存設定' : 'Saved configuration'}: ${targetName}`;
  if (feature.anchor === 'feature-calibration') return `${ja ? '実行対象' : 'Runtime target'}: ${runtimeTargetName || (ja ? '未選択 — 稼働一覧で対象を選択' : 'None selected — choose a running capture')}`;
  if (feature.destination === 'capturers') return ja ? '稼働一覧 — 個別操作と全体操作' : 'Running captures — individual and all-capture controls';
  if (feature.destination === 'logs') return ja ? 'ログ一覧 — 個別・統合の対象を選択' : 'Logs — select individual or combined scope';
  return ja ? 'アプリ全体の設定' : 'Application-wide preferences';
 }
</script>
<nav class="preview-feature-index" class:compact aria-label={language.startsWith('ja') ? '編集画面の機能目次' : 'Feature index in editor'}>
 {#if compact}
 <details class="compact-context"><summary>{language.startsWith('ja') ? '保存先・起動指定・次回既定' : 'Save, launch and next-start targets'}</summary>
  <dl>
   <dt>{language.startsWith('ja') ? '編集中の保存設定' : 'Editing'}</dt><dd>{targetName}</dd>
   <dt>{language.startsWith('ja') ? '共通の起動指定' : 'Launch target'}</dt><dd>{launchTargetName || '—'}</dd>
   <dt>{language.startsWith('ja') ? '次回起動時の選択' : 'Next-start selection'}</dt><dd>{savedDefaultName || '—'}</dd>
  </dl>
  <p>{language.startsWith('ja') ? `変更は自動保存。稼働中の関連個体 ${matchingRuntimeCount} 件へ同期する。反映時には入力処理の再起動を伴う。対象の選択自体では稼働個体は切り替わらない。` : `Changes save automatically and sync to ${matchingRuntimeCount} matching running captures. Applying changes restarts capture processing. Selecting a target does not switch running captures.`}</p>
 </details>
 {:else}
 <div class="target">{language.startsWith('ja') ? '編集中の保存設定' : 'Editing saved configuration'}: {targetName}</div>
 <dl class="target-context" aria-label={language.startsWith('ja') ? '対象の区別' : 'Target context'}>
  <div><dt>{language.startsWith('ja') ? '起動ボタンの対象' : 'Launch button target'}</dt><dd>{launchTargetName || '—'}</dd></div>
  <div><dt>{language.startsWith('ja') ? 'アプリ再起動後の選択' : 'Selection after app restart'}</dt><dd>{savedDefaultName || '—'}</dd></div>
 </dl>
 <details class="target-help"><summary>{language.startsWith('ja') ? 'この3つはどう違う？' : 'How do these targets differ?'}</summary>
  <p>{language.startsWith('ja') ? '編集中は設定を書き換える対象。起動ボタンの対象はキャプチャー画面で選びます。アプリ再起動後の選択は、最後に保存できた編集対象です。選択の保存が失敗した場合は以前の対象が残ります。起動中の個体を切り替える操作ではありません。' : 'Editing identifies the configuration being changed. Choose the launch button target on the Captures page. The selection restored after app restart is the last successfully saved editor selection. If saving fails, the previous selection remains. These selections do not switch running captures.'}</p>
 </details>
 <div class="target">{language.startsWith('ja')
  ? matchingRuntimeCount > 0
   ? `変更は自動保存。この設定を使う稼働個体 ${matchingRuntimeCount} 件へ反映を送ります。`
   : '変更は自動保存。この設定を使う稼働個体はありません。次の起動に使います。'
  : matchingRuntimeCount > 0
   ? `Changes save automatically and are sent to ${matchingRuntimeCount} running captures using this profile.`
   : 'Changes save automatically for the next launch. No running captures use this profile.'}</div>
 {/if}
 <div class="groups">
  {#each featureGroups as group}
   <details ontoggle={(event) => {
    const detail = event.currentTarget;
    const entries = detail.querySelector<HTMLElement>('.entries');
    if (detail.open && entries) entries.style.maxHeight = `${Math.max(60, window.innerHeight - detail.getBoundingClientRect().bottom - 20)}px`;
   }}>
    <summary>{featureText(group.title, language)}</summary>
    <div class="entries">
     {#each motionFeatures.filter(feature => feature.group === group.id) as feature}
      <button type="button" data-editor-feature={feature.id} title={featureText(feature.settings, language)} onclick={(event) => { const menu = event.currentTarget.closest("details"); if (menu) menu.open = false; void onOpen(feature); }}>
       <strong>{featureText(feature.title, language)}</strong>
       <small class="scope">{scope(feature)}</small>
       <small>{featureText(feature.settings, language)}</small>
      </button>
     {/each}
    </div>
   </details>
  {/each}
 </div>
</nav>
<style>
 .target-context{display:flex;flex-wrap:wrap;gap:8px 24px;margin:8px 0;font-size:12px}.target-context div{min-width:0}.target-context dt{color:var(--muted);font-size:11px}.target-context dd{margin:2px 0 0;overflow-wrap:anywhere;color:var(--text)}
 .target-help{font-size:11px;color:var(--muted);margin:6px 0}.target-help p{max-width:70ch;line-height:1.6}
 .preview-feature-index{position:sticky;top:0;z-index:20;background:var(--panel);border-bottom:1px solid var(--border);padding:10px 0; margin-bottom:16px}
 .target{font-size:12px;color:var(--muted);margin-bottom:6px;overflow-wrap:anywhere}
 .groups{display:flex;gap:8px;flex-wrap:wrap}
 details{position:relative;flex:1;min-width:130px}
 summary{cursor:pointer;padding:8px 10px;border:1px solid var(--border);border-radius:8px;font-size:12px}
 details[open] summary{color:var(--accent);border-color:var(--accent)}
 .entries{position:absolute;top:100%;left:0;width:min(340px,75vw);max-height:55vh;overflow:auto;background:var(--panel);border:1px solid var(--border);border-radius:8px;box-shadow:0 8px 24px #0006;padding:6px}
 details:last-child .entries{left:auto;right:0}
 button{display:block;width:100%;text-align:left;padding:10px;border:0;border-radius:6px;background:transparent;color:var(--text)}
 button:hover,button:focus-visible{background:var(--accent-soft)}
 strong,small{display:block}strong{font-size:13px}small{font-size:11px;color:var(--muted);margin-top:4px;line-height:1.4}
 .scope{color:var(--text);overflow-wrap:anywhere}
</style>
