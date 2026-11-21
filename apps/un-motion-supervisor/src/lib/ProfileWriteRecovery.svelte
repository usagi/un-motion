<script lang="ts">
 import {canRetryProfileWrite, type ProfileWrite} from './profileWriteRecovery';
 let {writes, japanese, onRetry, onDiscard, onReview}: {
  writes: ProfileWrite[]; japanese: boolean;
  onRetry: (write: ProfileWrite) => void; onDiscard: (id: number) => void;
  onReview: (write: ProfileWrite) => void;
 } = $props();
</script>

{#if writes.some(w => w.state === 'failed' || w.recovery)}
 <section class="write-recovery" aria-label={japanese ? '保存を確認できなかった入力' : 'Input with unconfirmed save'}>
  <strong>{japanese ? '保存を確認できなかった入力を保持しています' : 'Input with an unconfirmed save has been retained'}</strong>
  <p>{japanese ? '対象と保存内容を確認してから再保存してください。通信が途切れた場合は既に保存されている可能性があります。保持はこのウィンドウを閉じるまでです。' : 'Check the target and saved values before retrying. A lost connection can hide a successful save. Retained until this window closes.'}</p>
  {#each writes.filter(w => w.state === 'failed' || w.recovery) as write (write.id)}
   <details>
    <summary>{write.profileName} ({write.profileId}) · {Object.keys(write.fields).join(', ')} · {write.state === 'saving' ? (japanese ? '保存中' : 'Saving') : (japanese ? '保存未確認' : 'Save unconfirmed')}</summary>
    {#if write.superseded}<p>{japanese ? 'この入力の後に同じ項目を編集しています。古い値を上書きしないよう、再保存を無効にしています。内容は参照できます。' : 'A newer edit overlaps these fields. Retry is disabled to prevent restoring older values; the input remains available for reference.'}</p>{/if}
    {#if write.error.startsWith('input_validation:')}
     <p>{japanese ? '入力値が無効なため保存していません。入力した対象を開いて値を修正してください。元の入力はここで確認・破棄できます。' : 'The value is invalid and has not been saved. Open its profile to correct it. The original input can be reviewed or discarded here.'}</p>
     <button type="button" onclick={() => onReview(write)}>{japanese ? '入力した対象を開く' : 'Open input profile'}</button>
    {:else if write.error.includes('profile_conflict')}
     <p>{japanese ? '保存内容が別の操作で変更されています。最新の内容と保持した入力を比較し、必要な項目を新しい編集として入力してください。再確認だけでは上書きしません。' : 'Saved values changed elsewhere. Compare the latest values with this retained input, then enter the desired changes as a new edit. Reviewing does not overwrite anything.'}</p>
     <button type="button" disabled={write.state === 'saving'} onclick={() => onReview(write)}>{japanese ? '最新の保存内容を確認' : 'Review latest saved values'}</button>
    {:else}
     <button type="button" disabled={!canRetryProfileWrite(write)} onclick={() => onRetry(write)}>{japanese ? 'この対象へ再保存' : 'Save again to this profile'}</button>
    {/if}
    <button type="button" disabled={write.state === 'saving'} onclick={() => onDiscard(write.id)}>{japanese ? '保持した入力を破棄' : 'Discard retained input'}</button>
    <details><summary>{japanese ? '保持した入力と診断情報' : 'Retained input and diagnostic details'}</summary>
     <pre>{JSON.stringify(write.fields, null, 2)}</pre>
     <pre>{write.error}</pre>
    </details>
   </details>
  {/each}
 </section>
{/if}

<style>
 .write-recovery { margin: 1rem; padding: 1rem; border: 1px solid var(--accent-warn, #d97706); border-radius: .5rem; }
 p { margin: .5rem 0; } details { margin-top: .75rem; } summary { cursor: pointer; overflow-wrap: anywhere; }
 pre { min-height: 0; max-height: 12rem; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; }
 button { margin: .5rem .5rem 0 0; }
</style>
