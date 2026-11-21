import fs from 'node:fs';
import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
const source=fs.readFileSync(new URL('../src/App.svelte',import.meta.url),'utf8').split('<script lang="ts">')[1].split('</script>')[0];
const ast=ts.createSourceFile('App.ts',source,ts.ScriptTarget.Latest,true);
const names=['loadBackendSettings','syncAppSettingsToBackend','discardAppSettingsDraft','setAppSetting','guardProfileWriteUnload'];
const methods=names.map(name=>{const fn=ast.statements.find(n=>ts.isFunctionDeclaration(n)&&n.name?.text===name);assert.ok(fn,name);return fn.getText(ast);}).join('\n');
function fixture(invoke,ready=true){
 const code=ts.transpileModule(`
 let appSettings={themeMode:'system',locale:'ja-JP'},defaultAppSettings={...appSettings};
 let appSettingsReady=${ready},appSettingsLoading=false,appSettingsEarlyEdits={},appSettingsBeforeEarlyEdits=null,appSettingsPending=0,appSettingsSaveError='',appSettingsSaveSequence=0,appSettingsSaveTail=Promise.resolve();
 let profileWrites=[],busy=false,profileOrderDraft=null,profileOrderSaving=false,profilePointerDrag=null;function setUiLocale(){}
 ${methods}
 return {loadBackendSettings,syncAppSettingsToBackend,discardAppSettingsDraft,setAppSetting,guardProfileWriteUnload,
 snapshot:()=>({settings:appSettings,pending:appSettingsPending,error:appSettingsSaveError}),
 idle:()=>appSettingsSaveTail};`,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 return new Function('invoke',code)(invoke);
}
const tick=()=>new Promise(resolve=>setImmediate(resolve));
function guarded(f){const event=new Event('beforeunload',{cancelable:true});f.guardProfileWriteUnload(event);return event.defaultPrevented;}
test('serial writes retain newer edits when older save succeeds; latest failure retains input',async()=>{
 const calls=[];const f=fixture((cmd,args)=>new Promise((resolve,reject)=>calls.push({cmd,args,resolve,reject})));
 f.setAppSetting('themeMode','light');f.setAppSetting('themeMode','dark');
 assert.equal(guarded(f),true);await tick();assert.equal(calls.length,1);
 calls[0].resolve(calls[0].args.settings);await tick();
 assert.equal(f.snapshot().settings.themeMode,'dark');assert.equal(calls.length,2);
 calls[1].reject(Error('save failed'));await f.idle();
 assert.equal(f.snapshot().settings.themeMode,'dark');assert.match(f.snapshot().error,/save failed/);assert.equal(guarded(f),true);
 const retry=f.syncAppSettingsToBackend(f.snapshot().settings);await tick();
 assert.equal(calls[2].args.settings.themeMode,'dark');calls[2].resolve(calls[2].args.settings);await retry;
 assert.equal(guarded(f),false);assert.equal(f.snapshot().error,'');
});
test('failed reload retains draft; successful explicit reload releases protection',async()=>{
 let failRead=true;
 const f=fixture(async(cmd)=>{if(cmd==='sync_app_settings')throw Error('write failed');if(failRead)throw Error('read failed');return {themeMode:'system',locale:'ja-JP'};});
 f.setAppSetting('themeMode','dark');await f.idle();
 await f.discardAppSettingsDraft();assert.equal(f.snapshot().settings.themeMode,'dark');assert.equal(guarded(f),true);
 failRead=false;await f.discardAppSettingsDraft();assert.equal(f.snapshot().settings.themeMode,'system');assert.equal(guarded(f),false);
});
test('readback cannot overwrite an edit started while reload was waiting',async()=>{
 let finishRead;
 const f=fixture((cmd,args)=>cmd==='get_app_settings'?new Promise(resolve=>finishRead=resolve):Promise.resolve(args.settings));
 const reading=f.discardAppSettingsDraft();f.setAppSetting('themeMode','dark');await f.idle();
 finishRead({themeMode:'system'});await reading;assert.equal(f.snapshot().settings.themeMode,'dark');assert.equal(guarded(f),false);
});

test('startup edits merge with loaded settings, not defaults, and latest input wins',async()=>{
 let read;const writes=[];const f=fixture((cmd,args)=>cmd==='get_app_settings'?new Promise(resolve=>read=resolve):Promise.resolve((writes.push(args.settings),args.settings)),false);
 const loading=f.loadBackendSettings();f.setAppSetting('themeMode','light');f.setAppSetting('themeMode','dark');
 assert.equal(guarded(f),true);assert.equal(writes.length,0);
 read({themeMode:'system',locale:'ja-JP',autoLaunchSelectedOnStartup:false});await loading;await f.idle();
 assert.equal(writes.length,1);assert.equal(writes[0].themeMode,'dark');assert.equal(writes[0].autoLaunchSelectedOnStartup,false);assert.equal(guarded(f),false);
});
test('startup read failure cannot write defaults; retry retains early edits',async()=>{
 let fail=true;const writes=[];const f=fixture(async(cmd,args)=>{
  if(cmd==='get_app_settings'){if(fail)throw Error('locked');return {themeMode:'system',locale:'ja-JP',stopCapturersOnExit:false};}
  writes.push(args.settings);return args.settings;
 },false);
 f.setAppSetting('themeMode','dark');await tick();assert.equal(writes.length,0);assert.equal(guarded(f),true);
 fail=false;await f.syncAppSettingsToBackend(f.snapshot().settings);await f.idle();
 assert.equal(writes.length,1);assert.equal(writes[0].themeMode,'dark');assert.equal(writes[0].stopCapturersOnExit,false);assert.equal(guarded(f),false);
});

test('read failure without edits permits closing for restart',async()=>{
 const f=fixture(async()=>{throw Error('repair file and restart')},false);
 await f.loadBackendSettings();assert.match(f.snapshot().error,/restart/);assert.equal(guarded(f),false);
 f.setAppSetting('themeMode','dark');await tick();assert.equal(guarded(f),true);
});

test('explicit discard of early edits releases restart without touching unread file',async()=>{
 const calls=[];const f=fixture(async(cmd)=>{calls.push(cmd);throw Error('restart required')},false);
 f.setAppSetting('themeMode','dark');await tick();assert.equal(guarded(f),true);
 const before=calls.length;await f.discardAppSettingsDraft();assert.equal(calls.length,before);
 assert.equal(guarded(f),false);assert.equal(f.snapshot().settings.themeMode,'system');assert.match(f.snapshot().error,/restart/);
});

test('automatic startup waits for authoritative settings instead of an early toggle',async()=>{
 const method=ast.statements.find(n=>ts.isFunctionDeclaration(n)&&n.name?.text==='maybeAutoLaunchSelectedOnStartup').getText(ast);
 const compiled=ts.transpileModule(method,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 const f=new Function(`let busy=false,appSettingsReady=false,startupAutoLaunchAttempted=false,activeTab='settings',launches=0;
 const appSettings={autoLaunchSelectedOnStartup:true},launchTargetId='profile-a',profiles=[],isValidLaunchTarget=()=>true,liveCapturers=()=>[],launchCapturer=async()=>{launches++};
 ${compiled};return {run:maybeAutoLaunchSelectedOnStartup,ready:()=>appSettingsReady=true,state:()=>({launches,startupAutoLaunchAttempted,activeTab})}`)();
 await f.run();assert.deepEqual(f.state(),{launches:0,startupAutoLaunchAttempted:false,activeTab:'settings'});
 f.ready();await f.run();await f.run();
 assert.deepEqual(f.state(),{launches:1,startupAutoLaunchAttempted:true,activeTab:'capturers'});
});
