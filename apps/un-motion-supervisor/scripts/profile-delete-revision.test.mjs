import assert from 'node:assert/strict';
import test from 'node:test';
import {readFile} from 'node:fs/promises';
import ts from 'typescript';
const source=await readFile(new URL('../src/App.svelte',import.meta.url),'utf8');
function body(name){const match=new RegExp(`\\t(?:async )?function ${name}\\(`).exec(source);assert.ok(match);return source.slice(match.index,source.indexOf('\n\t}',match.index)+3);}
function harness(reject=false,fullRevision=false){
 const calls=[];let tick;let now=0;
 const code=`let selectedProfileId='a',profileDetail={id:'a',revision:'seen',deletionRevision:${fullRevision?"'delete:full-snapshot'":"undefined"}},busy=false,message='',errorMessage='',profileWrites=[];
 let deleteHoldTargetId=null,deleteHoldProgress=0,deleteHoldTimer=null,deleteHoldStartedAt=0;const deleteHoldDurationMs=1200;
 async function refreshProfiles(){}
 ${body('startDeleteHold')}${body('cancelDeleteHold')}${body('deleteSelectedProfile')}
 return {startDeleteHold,deleteSelectedProfile,change:()=>{profileDetail={id:'a',revision:'newer'}},state:()=>({selectedProfileId,profileDetail,errorMessage})};`;
 const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 const h=new Function('window','performance','invoke',js)({requestAnimationFrame:fn=>{tick=fn;return 1},cancelAnimationFrame:()=>{}},{now:()=>now},async(cmd,args)=>{calls.push({cmd,args});if(reject)throw Error('profile_conflict');});
 return {...h,calls,finish:async()=>{now=1200;tick();await new Promise(resolve=>setImmediate(resolve));}};
}
test('hold retains revision shown at its start despite a later detail refresh',async()=>{const h=harness();h.startDeleteHold('a');h.change();await h.finish();assert.equal(h.calls[0].args.expectedRevision,'seen');assert.equal(h.calls[0].args.profileId,'a');});
test('conflicting deletion retains selected profile and error',async()=>{const h=harness(true);await h.deleteSelectedProfile('seen');assert.equal(h.state().selectedProfileId,'a');assert.equal(h.state().profileDetail.revision,'seen');assert.match(h.state().errorMessage,/profile_conflict/);});
test('hold uses the complete deletion snapshot instead of field-write revision',async()=>{const h=harness(false,true);h.startDeleteHold('a');h.change();await h.finish();assert.equal(h.calls[0].args.expectedRevision,'delete:full-snapshot');});
test('reload refuses pending or failed edits for the selected target, but allows unrelated recovery',async()=>{
 for(const writes of [[{profileId:'a',state:'saving'}],[{profileId:'a',state:'failed'}],[{profileId:'b',state:'failed'}],[]]){
  const calls=[];const js=ts.transpileModule(`let selectedProfileId='a',busy=false,errorMessage='previous',profileWrites=writes;${body('reloadSelectedProfile')}return reloadSelectedProfile;`,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const reload=new Function('writes','refreshProfileDetail',js)(writes,async id=>calls.push(id));await reload();
  assert.deepEqual(calls,writes.some(w=>w.profileId==='a')?[]:['a']);
 }
});
