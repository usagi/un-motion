// Run with node --conditions=browser --test scripts/profile-default-effect.test.mjs
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { flushSync, untrack } from 'svelte';
import { mutable_source, get, set, effect_root, render_effect } from 'svelte/internal/client';

const app = (await readFile(new URL('../src/App.svelte', import.meta.url), 'utf8')).replaceAll('\r\n', '\n');
const start = app.indexOf('\t$effect(() => {\n\t\tconst profileId = selectedProfileId;');
assert.ok(start >= 0);
const effectBody = app.slice(start + '\t$effect(() => {'.length, app.indexOf('\n\t});', start));

test('external default refresh and save acknowledgements do not overwrite the shared default', () => {
  const selected = mutable_source('a');
  const ready = mutable_source(false);
  const persisted = mutable_source('a');
  const writes = [];
  const scope = {
    get selectedProfileId() { return get(selected); },
    get profilesReady() { return get(ready); },
    get persistedSelectedProfileId() { return get(persisted); },
    untrack,
    persistSelectedProfileId(id) { if (id !== get(persisted)) writes.push(id); },
  };
  // Execute the actual App effect against Svelte signals, including its tracked reads.
  const run = new Function('scope', `with (scope) { ${effectBody} }`);
  const dispose = effect_root(() => render_effect(() => run(scope)));
  try {
    flushSync(() => set(ready, true));
    assert.deepEqual(writes, []);
    flushSync(() => set(persisted, 'b'));
    assert.deepEqual(writes, [], 'reading another editor default must not save a');
    flushSync(() => set(selected, 'c'));
    assert.deepEqual(writes, ['c'], 'explicit editor selection still remembers c');
    flushSync(() => set(persisted, 'c'));
    flushSync(() => set(persisted, 'b'));
    assert.deepEqual(writes, ['c'], 'acknowledgements and refreshes must not trigger another save');
    flushSync(() => set(selected, 'b'));
    assert.deepEqual(writes, ['c'], 'selecting the saved default needs no write');
  } finally {
    dispose();
  }
});
