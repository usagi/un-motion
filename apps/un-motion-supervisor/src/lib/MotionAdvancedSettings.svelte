<script lang="ts">
  import { locale } from 'svelte-i18n';
  interface Pipeline {
    inputFfmpegPath: string | null;
    inputResizeEnabled: boolean | null;
    inputResizeAxis: string | null;
    inputResizeReference: number | null;
    inputResizeWidth: number | null;
    inputResizeHeight: number | null;
    inputResizePreserveAspect: boolean | null;
    inputResizePadColor: string | null;
    postProcess: string | null;
  }
  let { section, pipeline, hold, source, onUpdate, onInvalid }: {
    section: 'input' | 'processing'; pipeline: Pipeline; hold: boolean | null;
    source: string; onUpdate: (field: string, value: unknown) => void;
    onInvalid: (field: string, value: string, error: string) => void;
  } = $props();
  const tr = (ja: string, en: string) => ($locale ?? 'ja').startsWith('ja') ? ja : en;
  const update = (field: string, value: unknown) => onUpdate(`pipeline_components.${field}`, value);
  function validInput(field: string, input: HTMLInputElement): boolean {
    if (input.reportValidity()) return true;
    onInvalid(`pipeline_components.${field}`, input.value, input.validationMessage);
    return false;
  }
  const numbers = [
    { key: 'inputResizeReference', field: 'input_resize_reference', ja: '基準サイズ（px）', en: 'Reference size (px)' },
    { key: 'inputResizeWidth', field: 'input_resize_width', ja: '出力枠の幅（px）', en: 'Output width (px)' },
    { key: 'inputResizeHeight', field: 'input_resize_height', ja: '出力枠の高さ（px）', en: 'Output height (px)' },
  ] as const;
</script>

<details class="motion-advanced" data-motion-advanced={section}>
  <summary>
    {#if section === 'input'}
      {tr('入力の詳細', 'Input details')} · FFmpeg: {pipeline.inputFfmpegPath ? tr('専用指定', 'Profile override') : tr('共通設定', 'Shared setting')}
      · {tr('静止画像拡縮', 'Image resize')}: {pipeline.inputResizeEnabled ? 'ON' : 'OFF'}
    {:else}
      {tr('後処理とホールドの詳細', 'Postprocessing and hold details')} · {tr('後処理', 'Postprocess')}: {pipeline.postProcess === 'none' ? 'OFF' : 'ON'}
      · {tr('ホールド', 'Hold')}: {hold === false ? 'OFF' : 'ON'}
    {/if}
  </summary>
  <p>{tr('このプロファイルへ自動保存する。稼働中の同じプロファイルへの反映結果は保存通知で確認できる。', 'Automatically saved to this profile. Save feedback reports delivery to running instances of the same profile.')}</p>
  <div class="advanced-fields">
    {#if section === 'input'}
      <label class="wide">{tr('専用FFmpeg（空欄なら共通設定）', 'Profile FFmpeg (empty uses shared setting)')}
        <input type="text" value={pipeline.inputFfmpegPath ?? ''} onchange={e => update('input_ffmpeg_path', e.currentTarget.value.trim() || null)} />
      </label>
      <button type="button" onclick={() => update('input_ffmpeg_path', null)}>{tr('共通設定を使う', 'Use shared setting')}</button>
      <p class="wide">{source === 'file-image' ? tr('拡縮は現在の静止画像入力に適用する。', 'Resizing applies to the current still-image input.') : tr('拡縮は静止画像入力専用。現在の入力には適用しない。', 'Resizing applies only to still images, not the current input.')}</p>
      <label><input type="checkbox" checked={pipeline.inputResizeEnabled ?? false} onchange={e => update('input_resize_enabled', e.currentTarget.checked)} />{tr('静止画像を拡縮する', 'Resize still images')}</label>
      <label>{tr('基準の方向', 'Reference axis')}
        <select value={pipeline.inputResizeAxis ?? 'width'} onchange={e => update('input_resize_axis', e.currentTarget.value)}>
          <option value="width">{tr('幅', 'Width')}</option><option value="height">{tr('高さ', 'Height')}</option>
        </select>
      </label>
      {#each numbers as item}
        <label>{tr(item.ja, item.en)}
          <input type="number" min="1" max="4294967295" step="1" placeholder={tr('自動', 'Automatic')} value={pipeline[item.key] ?? ''}
            onchange={e => { if (validInput(item.field, e.currentTarget)) update(item.field, e.currentTarget.value === '' ? null : Number(e.currentTarget.value)); }} />
        </label>
      {/each}
      <label><input type="checkbox" checked={pipeline.inputResizePreserveAspect ?? true} onchange={e => update('input_resize_preserve_aspect', e.currentTarget.checked)} />{tr('縦横比を保つ', 'Preserve aspect ratio')}</label>
      <label>{tr('余白の色（RGBA・16進8桁）', 'Padding color (RGBA, 8 hex digits)')}
        <input type="text" pattern="[0-9a-fA-F]{8}" title={tr('0〜9・a〜fの8文字。空欄なら既定色。', 'Eight hexadecimal digits (0–9, a–f), or empty for the default.')} placeholder="000000ff" value={pipeline.inputResizePadColor ?? ''}
          onchange={e => { if (validInput('input_resize_pad_color', e.currentTarget)) update('input_resize_pad_color', e.currentTarget.value || null); }} />
      </label>
    {:else}
      <label>{tr('MediaPipeの後処理', 'MediaPipe postprocessing')}
        <select value={pipeline.postProcess ?? ''} onchange={e => update('post_process', e.currentTarget.value || null)}>
          <option value="">{tr('既定を使う', 'Use default')}</option>
          <option value="media-pipe-default">{tr('標準の後処理', 'Standard postprocessing')}</option>
          <option value="none">{tr('後処理を使わない', 'No postprocessing')}</option>
          {#if pipeline.postProcess && !['none', 'media-pipe-default'].includes(pipeline.postProcess)}<option value={pipeline.postProcess}>{pipeline.postProcess}</option>{/if}
        </select>
      </label>
      <label><input type="checkbox" checked={hold !== false} onchange={e => onUpdate('runtime_selection.modifier.post_process_rules.hold_lost_landmarks', e.currentTarget.checked)} />{tr('見失った姿勢のホールドを許可する', 'Allow holding lost poses')}</label>
      <p class="wide">{tr('許可を外すと、部位別にホールドを選んでも姿勢を保持しない。後処理を無効にすると、その後処理による補正も働かない。', 'Disabling hold overrides per-part hold choices. Disabling postprocessing also bypasses its corrections.')}</p>
    {/if}
  </div>
</details>

<style>
  .motion-advanced { min-width: 0; grid-column: 1 / -1; padding: 12px; border: 1px solid var(--border-color, #7777); border-radius: 10px; }
  summary { cursor: pointer; overflow-wrap: anywhere; }
  p { font-size: .9em; }
  .advanced-fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 230px), 1fr)); gap: 12px; }
  label { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; min-width: 0; }
  input:not([type=checkbox]), select { width: 100%; min-width: 0; box-sizing: border-box; }
  .wide { grid-column: 1 / -1; }
</style>
