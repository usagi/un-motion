import fs from 'node:fs';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const src=fs.readFileSync(new URL('../src/App.svelte',import.meta.url),'utf8').split('<script lang="ts">')[1].split('</script>')[0];
const ast=ts.createSourceFile('App.ts',src,ts.ScriptTarget.Latest,true);
const names=['retainProfileOrder','saveProfilesOrder','discardProfileOrder','cancelProfilePointerDrag','guardProfileWriteUnload'];
const methods=names.map(name=>ast.statements.find(n=>ts.isFunctionDeclaration(n)&&n.name?.text===name).getText(ast)).join('\n');
function fixture(invoke){
 const code=ts.transpileModule(`let profiles=[{id:'b'},{id:'a'}],profileOrderDraft=null,profileOrderSaving=false,profileDragOriginal=null,profilePointerDrag=null,draggedProfileId=null;
 let profileListRequest=0;let profileWrites=[],busy=false,appSettingsPending=0,appSettingsSaveError='',message='';function removeProfileDragListeners(){}
 ${methods}
 return {refresh:incoming=>{profiles=retainProfileOrder(incoming);},saveProfilesOrder,discardProfileOrder,cancelProfilePointerDrag,guardProfileWriteUnload,snapshot:()=>({profiles,draft:profileOrderDraft,saving:profileOrderSaving}),startDrag:()=>{profileDragOriginal=[{id:'a'},{id:'b'}];profilePointerDrag={active:true};}};`,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 return new Function('invoke',code)(invoke);
}
const guard=f=>{const e=new Event('beforeunload',{cancelable:true});f.guardProfileWriteUnload(e);return e.defaultPrevented;};
test('pending and failed order remain protected; retry sends retained order',async()=>{
 const calls=[];const f=fixture((cmd,args)=>new Promise((resolve,reject)=>calls.push({cmd,args,resolve,reject})));
 const saving=f.saveProfilesOrder();assert.equal(guard(f),true);await f.saveProfilesOrder();assert.equal(calls.length,1);
 calls[0].reject(Error('locked'));await saving;assert.deepEqual(f.snapshot().draft.ids,['b','a']);assert.equal(guard(f),true);
 const retry=f.saveProfilesOrder();assert.deepEqual(calls[1].args.profileIds,['b','a']);calls[1].resolve([]);await retry;assert.equal(guard(f),false);
});
test('failed reload retains order; successful reload releases guard',async()=>{
 let fail=true;const f=fixture(async cmd=>{if(cmd==='reorder_profiles'||fail)throw Error('failed');return [{id:'a'},{id:'b'}];});
 await f.saveProfilesOrder();await f.discardProfileOrder();assert.deepEqual(f.snapshot().draft.ids,['b','a']);assert.equal(guard(f),true);
 fail=false;await f.discardProfileOrder();assert.deepEqual(f.snapshot().profiles.map(x=>x.id),['a','b']);assert.equal(guard(f),false);
});
test('cancelled pointer drag restores original order without saving',()=>{
 const f=fixture(()=>{throw Error('must not write');});f.startDrag();assert.equal(guard(f),true);f.cancelProfilePointerDrag();assert.deepEqual(f.snapshot().profiles.map(x=>x.id),['a','b']);assert.equal(guard(f),false);
});

test('refresh preserves failed order with fresh metadata; membership change rejects retry',async()=>{
 let writes=0;const f=fixture(async()=>{writes++;throw Error('locked');});
 await f.saveProfilesOrder();
 f.refresh([{id:'a',name:'updated'},{id:'b'},{id:'c'}]);
 assert.deepEqual(f.snapshot().profiles.map(x=>x.id),['b','a','c']);
 assert.equal(f.snapshot().profiles[1].name,'updated');
 await f.saveProfilesOrder();assert.equal(writes,1);assert.equal(guard(f),true);
 assert.deepEqual(f.snapshot().draft.ids,['b','a']);
});
test('late readback cannot overwrite newer profile list',async()=>{
 let resolveRead;const f=fixture(cmd=>cmd==='reorder_profiles'?Promise.reject(Error('locked')):new Promise(resolve=>{resolveRead=resolve;}));
 await f.saveProfilesOrder();const reading=f.discardProfileOrder();
 f.refresh([{id:'a'},{id:'b'},{id:'new'}]);
 resolveRead([{id:'a'},{id:'b'}]);await reading;
 assert.deepEqual(f.snapshot().profiles.map(x=>x.id),['b','a','new']);
 assert.equal(guard(f),true);assert.match(f.snapshot().draft.error,/changed while reloading/);
});

test('retry retains original saved order despite incoming refreshed order',async()=>{
 const calls=[];const f=fixture(async(cmd,args)=>{calls.push(args);throw Error('conflict');});
 f.startDrag();await f.saveProfilesOrder();
 f.refresh([{id:'b'},{id:'a'}]);await f.saveProfilesOrder();
 assert.deepEqual(calls.map(c=>c.expectedIds),[['a','b'],['a','b']]);
 assert.equal(guard(f),true);
});
