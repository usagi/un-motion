import test from 'node:test';import assert from 'node:assert/strict';
import {effectiveSmoothing,smoothingMigration} from '../src/lib/smoothingMigration.ts';
import {ProfileSaveQueue} from '../src/lib/profileSaveQueue.ts';
const p='runtime_selection.modifier.';
const base={modifierSmoothingPreset:'high',modifierSmoothingEmaEnabled:null,modifierSmoothingOneEuroEnabled:null,modifierSmoothingEmaAlpha:null};
test('legacy high is shown active and disabling it atomically disables the fallback',()=>{
 assert.deepEqual(effectiveSmoothing(base),{ema:true,oneEuro:false,alpha:.25});
 assert.deepEqual(smoothingMigration(base,{[p+'smoothing_ema_enabled']:false}),{[p+'smoothing_preset']:'off',[p+'smoothing_ema_enabled']:false,[p+'smoothing_one_euro_enabled']:false,[p+'smoothing_ema_alpha']:.25});
});
test('explicit false switches do not conceal a legacy preset still used by runtime',()=>{
 assert.equal(effectiveSmoothing({...base,modifierSmoothingEmaEnabled:false,modifierSmoothingOneEuroEnabled:false}).ema,true);
});
test('enabling another stage preserves the effective legacy strength',()=>{
 const result=smoothingMigration(base,{[p+'smoothing_one_euro_enabled']:true});
 assert.equal(result[p+'smoothing_ema_alpha'],.25);assert.equal(result[p+'smoothing_ema_enabled'],true);assert.equal(result[p+'smoothing_one_euro_enabled'],true);
});
test('explicit active stages suppress the legacy preset exactly as runtime does',()=>{
 assert.deepEqual(effectiveSmoothing({...base,modifierSmoothingEmaEnabled:true,modifierSmoothingPreset:'ADAPTIVE',modifierSmoothingEmaAlpha:.8}),{ema:true,oneEuro:false,alpha:.8});
});
test('changing strength makes legacy value editable and leaves unrelated fields alone',()=>{
 assert.equal(smoothingMigration(base,{[p+'smoothing_ema_alpha']:.6})[p+'smoothing_ema_alpha'],.6);
 const fields={name:'other'};assert.equal(smoothingMigration(base,fields),fields);
});
test('explicit migration batch is stable for retry; off retains configured alpha',()=>{
 const fields=smoothingMigration(base,{[p+'smoothing_ema_enabled']:false});assert.equal(smoothingMigration(base,fields),fields);
 assert.equal(effectiveSmoothing({...base,modifierSmoothingPreset:'off',modifierSmoothingEmaAlpha:.8}).alpha,.8);
});
test('two edits during a pending migration retain both intentions through the actual save queue',async()=>{
 const queue=new ProfileSaveQueue();let release;const gate=new Promise(resolve=>release=resolve);
 let stored={},revision='old';const first=smoothingMigration(base,{[p+'smoothing_ema_enabled']:false});
 const one=queue.run('a','old',async expected=>{assert.equal(expected,'old');await gate;stored={...stored,...first};revision='first';return {revision};});
 const second=smoothingMigration(base,{[p+'smoothing_one_euro_enabled']:true},[first]);
 const two=queue.run('a','old',async expected=>{assert.equal(expected,'first');stored={...stored,...second};return {revision:'second'};});
 release();await Promise.all([one,two]);
 assert.equal(stored[p+'smoothing_ema_enabled'],false);assert.equal(stored[p+'smoothing_one_euro_enabled'],true);assert.equal(stored[p+'smoothing_preset'],'off');
});
test('pending alpha and adaptive edits survive normalization without copying unrelated settings',()=>{
 const fields=smoothingMigration(base,{[p+'smoothing_one_euro_enabled']:true},[{[p+'smoothing_ema_alpha']:.8,[p+'adaptive_beta']:.2,name:'other'}]);
 assert.equal(fields[p+'smoothing_ema_alpha'],.8);assert.equal(fields[p+'adaptive_beta'],.2);assert.equal(fields.name,undefined);
});
