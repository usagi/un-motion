import {smoothingMigration} from '../src/lib/smoothingMigration.ts';
import {ProfileSaveQueue} from '../src/lib/profileSaveQueue.ts';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/App.svelte', import.meta.url), 'utf8');

function defaultSaveHarness() {
 const pending=[];
 const invoke=(cmd,args)=>new Promise((resolve,reject)=>pending.push({cmd,args,resolve,reject}));
 const code=`let selectedProfileSaveQueue=Promise.resolve(),persistedSelectedProfileId='a',errorMessage='',selectedProfileSavePending=0,selectedProfileSaveFailure=null;
 ${body('persistSelectedProfileId')}
 return {save:persistSelectedProfileId,state:()=>({persistedSelectedProfileId,errorMessage,selectedProfileSavePending,selectedProfileSaveFailure})};`;
 const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 return {...new Function('invoke','console',js)(invoke,{warn(){}}),pending};
}

test('default saves serialize A to B to A including returning before B completes',async()=>{
 const h=defaultSaveHarness();
 await h.save('a');assert.equal(h.pending.length,0);
 const b=h.save('b');await Promise.resolve();
 const a=h.save('a');await Promise.resolve();
 assert.equal(h.pending.length,1);
 assert.equal(h.pending[0].args.profileId,'b');
 h.pending[0].resolve();await b;await Promise.resolve();
 assert.equal(h.pending.length,2);
 assert.equal(h.pending[1].args.profileId,'a');
 h.pending[1].resolve();await a;
 assert.equal(h.state().persistedSelectedProfileId,'a');
});

test('failed default save is visible and does not block the next selection',async()=>{
 const h=defaultSaveHarness();
 const b=h.save('b');const c=h.save('c');await Promise.resolve();
 h.pending[0].reject(Error('write denied'));await b;await Promise.resolve();
 assert.equal(h.state().persistedSelectedProfileId,'a');
 assert.match(h.state().selectedProfileSaveFailure.detail,/write denied/);
 assert.equal(h.pending[1].args.profileId,'c');
 h.pending[1].resolve();await c;
 assert.equal(h.state().persistedSelectedProfileId,'c');
 assert.equal(h.state().selectedProfileSaveFailure,null);
});

test('failed startup selection retains saved default and can retry the same target',async()=>{
 const h=defaultSaveHarness();const failed=h.save('b');await Promise.resolve();
 assert.equal(h.state().selectedProfileSavePending,1);
 h.pending[0].reject(Error('locked'));await failed;
 assert.equal(h.state().persistedSelectedProfileId,'a');assert.equal(h.state().selectedProfileSavePending,0);
 assert.equal(h.state().selectedProfileSaveFailure.profileId,'b');
 const retry=h.save('b');await Promise.resolve();assert.equal(h.pending[1].args.profileId,'b');
 h.pending[1].resolve();await retry;
 assert.equal(h.state().persistedSelectedProfileId,'b');assert.equal(h.state().selectedProfileSaveFailure,null);assert.equal(h.state().selectedProfileSavePending,0);
});
import { ProfileSyncFeedback, profileSyncWarning, savedProfileRefreshWarning } from '../src/lib/profileSync.ts';
import {beginProfileWrite, finishProfileWrite, canRetryProfileWrite} from '../src/lib/profileWriteRecovery.ts';

function body(name) {
 let start=source.indexOf(`\tasync function ${name}(`);
 if(start<0) start=source.indexOf(`\tfunction ${name}(`);
 assert.ok(start>=0);
 return source.slice(start,source.indexOf('\n\t}',start)+3);
}
function harness(refreshFailure = false, realProfileRefresh = false) {
 const pending=[];
 const invoke=(cmd,args)=>new Promise((resolve,reject)=>pending.push({cmd,args,resolve,reject}));
 const code=`const videoProbeRequests=new Map(); ${body('invalidateVideoProbe')} let appSettingsEarlyEdits={},appSettingsReady=true;let busy=false,appSettingsPending=0,appSettingsSaveError=null,profileOrderDraft=null,profileOrderSaving=false,profilePointerDrag=null;let profileWrites=[],profileWriteSequence=0; const profileSaveQueue=new ProfileSaveQueue(); const profileSyncFeedback=new ProfileSyncFeedback(); const appliedProfileSummaryWrites=new Map(); let $locale='ja-JP',selectedProfileId='a', profileDetail={id:'a'}, errorMessage='', profileDetailRequest=0, profiles=[{id:'a'},{id:'b'}];
 let profileListRequest=0,persistedSelectedProfileId='a',profilesReady=true,launchTargetId='a'; const retainProfileOrder=list=>list; const pickInitialLaunchTargetId=id=>id;
 ${realProfileRefresh ? body('refreshProfiles') : 'async function refreshProfiles(){}'} async function refreshRuntimeStatuses(){if(refreshFailure)throw Error('status unavailable');}
 let appSettings={}, message='', mediaPipeSourceType='file-video', webcamFormats=[];
 ${body('acceptProfileSummaryWrite')} ${body('startProfileWrite')} ${body('retryProfileWrite')} ${body('discardProfileWrite')} ${body('guardProfileWriteUnload')}
 ${body('refreshProfileDetail')} ${body('selectProfile')} ${body('updateProfileField')} ${body('updateProfileFields')} ${body('updateEngineType')} ${body('updateVideoInputPath')} ${body('browseInputFile')} ${body('updateCameraSetting')} ${body('browseProfileIcon')}
 return {selectProfile,refreshProfileDetail,updateProfileField,updateProfileFields,updateEngineType,updateVideoInputPath,browseInputFile,updateCameraSetting,browseProfileIcon,retryProfileWrite,discardProfileWrite,guardProfileWriteUnload,state:()=>({selectedProfileId,profileDetail,errorMessage,profiles,profileWrites})};`;
 const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 return {...new Function('smoothingMigration','ProfileSyncFeedback','ProfileSaveQueue','invoke','profileSyncWarning','beginProfileWrite','finishProfileWrite','canRetryProfileWrite','savedProfileRefreshWarning','refreshFailure',js)(smoothingMigration,ProfileSyncFeedback,ProfileSaveQueue,invoke,profileSyncWarning,beginProfileWrite,finishProfileWrite,canRetryProfileWrite,savedProfileRefreshWarning,refreshFailure),pending};
}

for (const batch of [false,true]) {
 for (const deliveryFailure of [false,true]) {
  test(`saved ${batch?'batch':'field'} with failed refresh retains delivery evidence=${deliveryFailure} without unsaved retry`,async()=>{
   const h=harness(true);
   const save=batch?h.updateProfileFields({note:'saved'}):h.updateProfileField('note','saved');
   h.pending[0].resolve({id:'a',name:'Alpha',note:'saved',runtime:{},syncReport:{discoveryError:null,
    targets:deliveryFailure?[{capturerId:12,accepted:false,error:'HTTP 500'}]:[]}});
   await save;
   assert.equal(h.state().profileWrites.length,0);
   assert.equal(h.state().profileDetail.note,'saved');
   assert.match(h.state().errorMessage,/保存済み/);assert.match(h.state().errorMessage,/再保存は不要/);
   assert.match(h.state().errorMessage,/status unavailable/);
   assert.equal(h.state().errorMessage.includes('HTTP 500'),deliveryFailure);
   assert.equal(h.pending.length,1);
  });
 }
}

test('failed name remains associated with A and retry while editing B saves only A',async()=>{
 const h=harness();const save=h.updateProfileField('name','retained name');
 const b=h.selectProfile('b');h.pending[1].resolve({id:'b',name:'Beta'});await b;
 h.pending[0].reject(Error('disk failure'));await save;
 const write=h.state().profileWrites[0];
 assert.deepEqual(write.fields,{name:'retained name'});assert.equal(write.profileId,'a');
 const retry=h.retryProfileWrite(write);
 assert.equal(h.pending[2].args.profileId,'a');
 assert.deepEqual(h.pending[2].args.updates,[{field:'name',value:'retained name'}]);
 h.pending[2].resolve({id:'a',name:'retained name',runtime:{}});await retry;
 assert.equal(h.state().profileDetail.name,'Beta');assert.equal(h.state().profileWrites.length,0);
});

test('overlapping newer edit prevents replay of an older failed atomic batch',async()=>{
 const h=harness();const batch=h.updateProfileFields({name:'old',note:'batch'});
 const newer=h.updateProfileField('name','new');
 assert.equal(h.pending.length,1);
 h.pending[0].reject(Error('old batch failure'));await batch;await Promise.resolve();
 h.pending[1].resolve({id:'a',name:'new',runtime:{}});await newer;
 const old=h.state().profileWrites[0];assert.equal(old.superseded,true);
 assert.deepEqual(old.fields,{name:'old',note:'batch'});
 await h.retryProfileWrite(old);assert.equal(h.pending.length,2);
 h.discardProfileWrite(old.id);assert.equal(h.state().profileWrites.length,0);
});

test('failed retry retains input once; double retry and discard during save are prevented',async()=>{
 const h=harness();const save=h.updateProfileField('note','keep');h.pending[0].reject(Error('offline'));await save;
 const old=h.state().profileWrites[0];const retry=h.retryProfileWrite(old);
 await h.retryProfileWrite(old);assert.equal(h.pending.length,2);
 h.discardProfileWrite(h.state().profileWrites[0].id);assert.equal(h.state().profileWrites.length,1);
 h.pending[1].reject(Error('still offline'));await retry;
 assert.equal(h.state().profileWrites.length,1);assert.deepEqual(h.state().profileWrites[0].fields,{note:'keep'});
});

test('an edit to another profile or unrelated field does not invalidate recovery',async()=>{
 const h=harness();const a=h.updateProfileField('note','A');h.pending[0].reject(Error('fail'));await a;
 const b=h.updateProfileField('note','B','b');h.pending[1].resolve({id:'b',runtime:{}});await b;
 const c=h.updateProfileField('name','A name','a');h.pending[2].resolve({id:'a',name:'A name',runtime:{}});await c;
 assert.equal(h.state().profileWrites[0].superseded,false);
});

test('recovery copies nested values and keeps a pending retry visible',()=>{
 const fields={value:{parts:[1,2]}};
 let writes=beginProfileWrite([],1,'a','Alpha',fields);
 fields.value.parts.push(3);
 assert.deepEqual(writes[0].fields,{value:{parts:[1,2]}});
 writes=finishProfileWrite(writes,1,'failure');
 writes=beginProfileWrite(writes,2,'a','Alpha',writes[0].fields,1);
 assert.equal(writes.length,1);assert.equal(writes[0].recovery,true);assert.equal(writes[0].state,'saving');
});

test('unload guard covers pending and failed saves until explicit discard',async()=>{
 const h=harness();let prevented=0;
 const event={preventDefault(){prevented++;},returnValue:undefined};
 h.guardProfileWriteUnload(event);assert.equal(prevented,0);
 const save=h.updateProfileField('note','keep');h.guardProfileWriteUnload(event);assert.equal(prevented,1);
 h.pending[0].reject(Error('offline'));await save;
 h.guardProfileWriteUnload(event);assert.equal(prevented,2);
 h.discardProfileWrite(h.state().profileWrites[0].id);
 h.guardProfileWriteUnload(event);assert.equal(prevented,2);
});
test('slow A response cannot replace selected B details',async()=>{
 const h=harness();const a=h.selectProfile('a');const b=h.selectProfile('b');
 h.pending[1].resolve({id:'b'});await b;h.pending[0].resolve({id:'a'});await a;
 assert.equal(h.state().selectedProfileId,'b');assert.equal(h.state().profileDetail.id,'b');
});
test('obsolete load failure cannot erase the new selection',async()=>{
 const h=harness();const a=h.selectProfile('a');const b=h.selectProfile('b');
 h.pending[1].resolve({id:'b'});await b;h.pending[0].reject(new Error('old request'));await a;
 assert.equal(h.state().profileDetail.id,'b');assert.equal(h.state().errorMessage,'');
});
test('selecting B clears editable A details until B arrives',async()=>{
 const h=harness();const b=h.selectProfile('b');assert.equal(h.state().profileDetail,null);
 h.pending[0].resolve({id:'b'});await b;assert.equal(h.state().profileDetail.id,'b');
});
test('newest request wins when returning to the same profile',async()=>{
 const h=harness();const a1=h.selectProfile('a');const b=h.selectProfile('b');const a2=h.selectProfile('a');
 h.pending[2].resolve({id:'a',version:2});await a2;
 h.pending[1].resolve({id:'b'});await b;h.pending[0].resolve({id:'a',version:1});await a1;
 assert.equal(h.state().profileDetail.version,2);
});
test('saving A preserves its ID and cannot overwrite B editor after selection changes',async()=>{
 const h=harness();const save=h.updateProfileField('note','A edited');const b=h.selectProfile('b');
 assert.equal(h.pending[0].args.profileId,'a');
 h.pending[1].resolve({id:'b',note:'B original'});await b;
 h.pending[0].resolve({id:'a',note:'A edited',runtime:{engine:'vmc'}});await save;
 assert.equal(h.state().profileDetail.id,'b');assert.equal(h.state().profileDetail.note,'B original');
 assert.equal(h.state().profiles.find(p=>p.id==='a').note,'A edited');
});

test('multi-field save sends one batch to A while selecting B',async()=>{
 const h=harness();const save=h.updateProfileFields({first:1,second:2});const b=h.selectProfile('b');
 h.pending[1].resolve({id:'b',note:'B original'});await b;
 assert.equal(h.pending[0].cmd,'update_profile_fields');
 assert.deepEqual(h.pending[0].args,{profileId:'a',updates:[{field:'first',value:1},{field:'second',value:2}]});
 h.pending[0].resolve({id:'a',runtime:{}});await save;
 assert.equal(h.pending.length,2);
 assert.equal(h.state().profileDetail.id,'b');
});

test('engine selection preserves inactive input settings and stays on A across profile change',async()=>{
 const h=harness();const save=h.updateEngineType('vmc');const b=h.selectProfile('b');
 h.pending[1].resolve({id:'b'});await b;
 assert.equal(h.pending[0].cmd,'update_profile_fields');
 assert.equal(h.pending[0].args.profileId,'a');
 assert.equal(h.pending[0].args.updates.length,1);
 assert.deepEqual(h.pending[0].args.updates.at(-1),{field:'runtime_selection.engine',value:'vmc'});
 h.pending[0].resolve({id:'a',runtime:{engine:'vmc'}});
 await save;assert.equal(h.state().profileDetail.id,'b');
 assert.equal(h.state().profiles.find(p=>p.id==='a').engine,'vmc');
});

test('all engine selections only change the engine, never clear another source configuration',async()=>{
 for(const engine of ['vmc','ifacialmocap','mediapipe-native']){
  const h=harness();const save=h.updateEngineType(engine);
  assert.deepEqual(h.pending[0].args.updates,[{field:'runtime_selection.engine',value:engine}]);
  h.pending[0].resolve({id:'a',runtime:{engine}});await save;
 }
});

test('video metadata arriving after selection change updates the originating profile',async()=>{
 const h=harness();const save=h.updateVideoInputPath('fixture.mp4');const b=h.selectProfile('b');
 assert.equal(h.pending[0].cmd,'probe_video_file_metadata');
 h.pending[1].resolve({id:'b'});await b;
 h.pending[0].resolve({fpsRounded:60,width:1920,height:1080,fps:60});await Promise.resolve();
 assert.equal(h.pending[2].cmd,'update_profile_fields');
 assert.equal(h.pending[2].args.profileId,'a');
 assert.equal(h.pending[2].args.updates.length,4);
 h.pending[2].resolve({id:'a',runtime:{}});
 await save;assert.equal(h.state().profileDetail.id,'b');
});


test('manual camera resolution sends one batch to A while selecting B',async()=>{
 const h=harness();const save=h.updateCameraSetting('1280x720');const b=h.selectProfile('b');
 h.pending[1].resolve({id:'b',note:'B original'});await b;
 assert.equal(h.pending[0].cmd,'update_profile_fields');
 assert.equal(h.pending[0].args.profileId,'a');
 assert(h.pending[0].args.updates.some(u=>u.field==='runtime_selection.resolution'&&u.value==='1280x720'));
 h.pending[0].resolve({id:'a',runtime:{}});await save;
 assert.equal(h.pending.length,2);
 assert.equal(h.state().profileDetail.id,'b');
 assert.equal(h.state().profileDetail.note,'B original');
});


test('profile icon picker retains its originating profile',async()=>{
 const h=harness();const save=h.browseProfileIcon();const b=h.selectProfile('b');
 h.pending[1].resolve({id:'b'});await b;
 h.pending[0].resolve('chosen.png');await Promise.resolve();
 assert.equal(h.pending[2].args.profileId,'a');
 h.pending[2].resolve({id:'a',runtime:{}});await save;
 assert.equal(h.state().profileDetail.id,'b');
});

function cameraHarness(){
 const pending=[];
 const invoke=(cmd,args)=>new Promise((resolve,reject)=>pending.push({cmd,args,resolve,reject}));
 const code=`let backend='directshow', webcamDevices=[], webcamFormats=[], webcamDevicesError='',webcamFormatsError='',webcamDevicesLoading=false,webcamFormatsLoading=false,webcamDevicesRequest=0,webcamFormatsRequest=0;
 function currentWebcamBackend(){return backend;}
 ${body('refreshWebcamDevices')} ${body('refreshWebcamFormats')}
 return {refreshWebcamDevices,refreshWebcamFormats,setBackend:b=>backend=b,state:()=>({webcamDevices,webcamFormats,webcamDevicesError,webcamFormatsError,webcamDevicesLoading,webcamFormatsLoading})};`;
 const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 return {...new Function('invoke',js)(invoke),pending};
}

for (const batch of [false, true]) {
 test(`saved ${batch ? 'batch' : 'field'} reports partial delivery for A without replacing B`, async () => {
  const h=harness();
  const save=batch?h.updateProfileFields({note:'saved'}):h.updateProfileField('note','saved');
  const b=h.selectProfile('b');h.pending[1].resolve({id:'b'});await b;
  h.pending[0].resolve({id:'a',name:'Alpha',note:'saved',runtime:{engine:'vmc'},syncReport:{discoveryError:null,targets:[
   {capturerId:11,accepted:true,error:null},{capturerId:12,accepted:false,error:'HTTP 500'}
  ]}});
  await save;
  assert.equal(h.state().profileDetail.id,'b');
  assert.equal(h.state().profiles.find(p=>p.id==='a').note,'saved');
  assert.match(h.state().errorMessage,/Alpha \(a\)/);
  assert.match(h.state().errorMessage,/保存済み/);
  assert.match(h.state().errorMessage,/Capturer #12: HTTP 500/);
  assert.doesNotMatch(h.state().errorMessage,/Capturer #11/);
  assert.equal(h.pending.length,2,'No automatic retry or resave');
 });
}

test('missing reports and accepted responses do not fabricate delivery failures',()=>{
 assert.equal(profileSyncWarning({id:'a',name:'A'},'ja-JP'),null);
 assert.equal(profileSyncWarning({id:'a',name:'A',syncReport:{targets:[],discoveryError:null}},'ja-JP'),null);
 assert.equal(profileSyncWarning({id:'a',name:'A',syncReport:{targets:[{capturerId:1,accepted:true,error:null}],discoveryError:null}},'ja-JP'),null);
 assert.match(profileSyncWarning({id:'a',name:'A',syncReport:{targets:[],discoveryError:'registry unavailable'}},'en-US'),/were saved.*registry unavailable/);
});
for(const kind of ['Devices','Formats']){
 test(`obsolete webcam ${kind} result cannot replace newer candidates`,async()=>{
  const h=cameraHarness();const refresh=h[`refreshWebcam${kind}`];
  const a=refresh('camera-a');const b=refresh('camera-b');
  h.pending[1].resolve(['new']);await b;
  h.pending[0].resolve(['old']);await a;
  assert.deepEqual(h.state()[`webcam${kind}`],['new']);
 });
 test(`obsolete webcam ${kind} failure cannot clear loading of newer request`,async()=>{
  const h=cameraHarness();const refresh=h[`refreshWebcam${kind}`];
  const a=refresh('camera-a');const b=refresh('camera-b');
  h.pending[0].reject(new Error('old'));await a;
  assert.equal(h.state()[`webcam${kind}Loading`],true);
  assert.equal(h.state()[`webcam${kind}Error`],'');
  h.pending[1].resolve(['new']);await b;
 });
 test(`leaving webcam input invalidates pending ${kind} enumeration`,async()=>{
  const h=cameraHarness();const refresh=h[`refreshWebcam${kind}`];
  const a=refresh('camera-a');h.setBackend(null);await refresh('');
  h.pending[0].resolve(['old']);await a;
  assert.deepEqual(h.state()[`webcam${kind}`],[]);
  assert.equal(h.state()[`webcam${kind}Loading`],false);
 });
}

for(const kind of ['Devices','Formats']){
 test(`old webcam ${kind} candidates are cleared while replacement is loading`,async()=>{
  const h=cameraHarness();const refresh=h[`refreshWebcam${kind}`];
  const a=refresh('camera-a');h.pending[0].resolve(['old']);await a;
  const b=refresh('camera-b');
  assert.deepEqual(h.state()[`webcam${kind}`],[]);
  assert.equal(h.state()[`webcam${kind}Loading`],true);
  h.pending[1].resolve(['new']);await b;
  assert.deepEqual(h.state()[`webcam${kind}`],['new']);
 });
}


function faceModelHarness(){
 const pending=[];
 const invoke=(cmd,args)=>new Promise((resolve,reject)=>pending.push({cmd,args,resolve,reject}));
 const code=`let busy=false,calibratingCapturerId=null,profileDetail={id:'a',name:'A'},selectedProfileId='a',capturers=[{id:11,profileId:'a',state:'running'}],selectedCapturerId=null,actionCountdownOverlay=null,message='',errorMessage='',appSettings={};
 let refreshCapturersPromise=null,capturerListGeneration=0,profileDetailRequest=0,capturerProfileDetailRequest=0,selectedCapturerProfileId=null,selectedCapturerProfileDetail=null,$locale='ja-JP';const profileSaveQueue={captureObserver(){return ()=>true;},observe(){}};
 ${body('finishCapturerProfileSave')}
 ${body('refreshCapturers')}
 async function refreshProfiles(){} async function refreshRuntimeStatuses(){}
 async function runActionCountdown(){} function faceModelOverlay(){}
 ${body('buildSelectedProfileFacePoseModel')}
 return {build:buildSelectedProfileFacePoseModel,selectB:()=>{selectedProfileId='b';profileDetail={id:'b',name:'B'};},state:()=>({busy,selectedProfileId,profileDetail,errorMessage})};`;
 const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 return {...new Function('invoke','profileSyncWarning','savedProfileRefreshWarning',js)(invoke,profileSyncWarning,savedProfileRefreshWarning),pending};
}
test('face model build reserves operation before refreshing capturers',async()=>{
 const h=faceModelHarness();const first=h.build();h.build();
 assert.equal(h.pending.length,1);
 h.pending[0].resolve([{id:11,profileId:'a',state:'running'}]);await new Promise(setImmediate);
 h.pending[1].resolve({id:'a',name:'A',runtime:{}});await first;
 assert.equal(h.state().busy,false);
});
test('face model completion preserves another selected profile',async()=>{
 const h=faceModelHarness();const build=h.build();h.selectB();
 h.pending[0].resolve([{id:11,profileId:'a',state:'running'}]);await new Promise(setImmediate);
 assert.equal(h.pending[1].cmd,'build_capturer_face_pose_model');
 assert.equal(h.pending[1].args.id,11);
 h.pending[1].resolve({id:'a',name:'A',runtime:{}});await build;
 assert.equal(h.state().selectedProfileId,'b');assert.equal(h.state().profileDetail.id,'b');
});

test('face model preparation failure releases operation lock for retry',async()=>{
 const h=faceModelHarness();const first=h.build();
 h.pending[0].reject(new Error('unavailable'));await new Promise(setImmediate);
 assert.equal(h.pending.length,1,'failed enumeration must not launch or calibrate a capturer');await first;
 assert.equal(h.state().busy,false);
 const retry=h.build();assert.equal(h.pending.length,2);
 h.pending[1].resolve([{id:11,profileId:'a',state:'running'}]);await new Promise(setImmediate);
 h.pending[2].resolve({id:'a',name:'A',runtime:{}});await retry;
 assert.equal(h.state().busy,false);
});

function deletionHarness(){
 const pending=[];const invoke=(cmd,args)=>cmd==='selected_profile_id'?Promise.resolve('b'):new Promise((resolve,reject)=>pending.push({cmd,args,resolve,reject}));
 const code=`const $_=s=>s;let profileWrites=[];let busy=false,selectedProfileId='a',profileDetail={id:'a'},profiles=[{id:'a'},{id:'b'}],profilesReady=true,persistedSelectedProfileId='a',launchTargetId='b',message='',errorMessage='';
 let detailReloads=[];const pickInitialLaunchTargetId=current=>current;
 async function refreshProfileDetail(id){detailReloads.push(id);profileDetail={id};}
 let profileListRequest=0;const retainProfileOrder=list=>list;${body('refreshProfiles')}${body('deleteSelectedProfile')}
 return {deleteSelectedProfile,pendingEdit(profileId,state){profileWrites.push({profileId,state});},selectB:()=>{selectedProfileId='b';profileDetail={id:'b',note:'unsaved B'};},state:()=>({errorMessage,profileWrites,selectedProfileId,profileDetail,launchTargetId,busy,detailReloads})};`;
 return {...new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(invoke),pending};
}
test('deleting A retains B selection and in-progress editor while refreshing the list',async()=>{
 const h=deletionHarness();const task=h.deleteSelectedProfile();h.selectB();assert.equal(h.pending[0].args.profileId,'a');h.pending[0].resolve();await Promise.resolve();
 assert.equal(h.pending[1].cmd,'list_profiles');h.pending[1].resolve([{id:'b'}]);await task;
 assert.deepEqual(h.state(),{errorMessage:'',profileWrites:[],selectedProfileId:'b',profileDetail:{id:'b',note:'unsaved B'},launchTargetId:'b',busy:false,detailReloads:[]});
});
test('deleting the selected profile loads the remaining valid target',async()=>{
 const h=deletionHarness();const task=h.deleteSelectedProfile();h.pending[0].resolve();await Promise.resolve();h.pending[1].resolve([{id:'b'}]);await task;
 assert.equal(h.state().selectedProfileId,'b');assert.deepEqual(h.state().detailReloads,['b']);
});
test('failed deletion retains another selected profile without list reload',async()=>{
 const h=deletionHarness();const task=h.deleteSelectedProfile();h.selectB();h.pending[0].reject(Error('in use'));await task;
 assert.equal(h.pending.length,1);assert.equal(h.state().profileDetail.note,'unsaved B');assert.equal(h.state().busy,false);
});

function restorationHarness(invoke) {
 const code=`let busy=false,message='Deleted a',errorMessage='Earlier operation failed',profiles=[{id:'b'}],selectedProfileId='b',profileDetail={id:'b',note:'unsaved'},persistedSelectedProfileId='b';
 let profileListRequest=0; const retainProfileOrder=list=>list; const $_=(key)=>key;
 ${body('restoreArchivedProfile')}
 return {restoreArchivedProfile,editSummary(){profiles=profiles.map(p=>({...p,note:'newer saved note'}));},saveDefault(){persistedSelectedProfileId='c';},state:()=>({busy,message,errorMessage,profiles,selectedProfileId,profileDetail,persistedSelectedProfileId})};`;
 return new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(invoke);
}
test('archive restoration refreshes list without replacing another profile draft',async()=>{
 const calls=[];
 const h=restorationHarness(async(cmd,args)=>{calls.push([cmd,args]);return cmd==='restore_deleted_profile'?{selectedProfileId:'b'}:[{id:'b'},{id:'a'}];});
 await h.restoreArchivedProfile('archive-a');
 assert.deepEqual(calls[0],['restore_deleted_profile',{archiveId:'archive-a'}]);
 assert.equal(h.state().profileDetail.note,'unsaved');assert.equal(h.state().selectedProfileId,'b');
 assert.equal(h.state().profiles.length,2);assert.equal(h.state().busy,false);
 assert.equal(h.state().message,'');assert.equal(h.state().errorMessage,'');
});
test('archive conflict does not refresh or discard the editor',async()=>{
 let calls=0;const h=restorationHarness(async()=>{calls++;throw new Error('conflict');});
 await assert.rejects(h.restoreArchivedProfile('archive-a'),/conflict/);
 assert.equal(calls,1);assert.equal(h.state().profileDetail.note,'unsaved');assert.equal(h.state().busy,false);
});
test('successful restore with failed list refresh reports partial success and retains editor',async()=>{
 const h=restorationHarness(async(cmd)=>{if(cmd==='restore_deleted_profile') return {selectedProfileId:'a'};throw new Error('unavailable');});
 await assert.rejects(h.restoreArchivedProfile('archive-a'),/profiles.archive.refresh_failed/);
 assert.equal(h.state().persistedSelectedProfileId,'a');assert.equal(h.state().selectedProfileId,'b');
 assert.equal(h.state().profileDetail.note,'unsaved');assert.equal(h.state().busy,false);
});

function creationHarness(){
 const pending=[];const invoke=(cmd,args)=>new Promise((resolve,reject)=>pending.push({cmd,args,resolve,reject}));
 const code=`const $_=s=>s;let profileWrites=[];let busy=false,message='',errorMessage='',selectedProfileId='a',profileDetail={id:'a'},profileDetailRequest=0,persistedSelectedProfileId='a',launchTargetId='running';let refreshArgument;
 async function refreshProfiles(value){refreshArgument=value;}
 let creatingProfile=false; ${body('createNewProfile')} ${body('duplicateSelectedProfile')}
 return {createNewProfile,duplicateSelectedProfile,pendingEdit(profileId,state){profileWrites.push({profileId,state});},select(id){selectedProfileId=id;profileDetail={id,note:'new draft'};profileDetailRequest++;},state:()=>({busy,errorMessage,profileWrites,selectedProfileId,profileDetail,refreshArgument,persistedSelectedProfileId,launchTargetId})};`;
 return {...new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(invoke),pending};
}
for(const action of ['createNewProfile','duplicateSelectedProfile']) {
 test(`${action} preserves editor after navigating to another profile`,async()=>{
  const h=creationHarness();const run=h[action]();h.select('b');h.pending[0].resolve({id:'new',name:'New'});await run;
  assert.equal(h.state().selectedProfileId,'b');assert.equal(h.state().profileDetail.note,'new draft');assert.equal(h.state().refreshArgument,false);
  assert.equal(h.state().persistedSelectedProfileId,action==='createNewProfile'?'new':'a');assert.equal(h.state().launchTargetId,'running');
  if(action==='duplicateSelectedProfile') assert.equal(h.pending[0].args.profileId,'a');
 });
 test(`${action} does not steal selection after A to B to A`,async()=>{
  const h=creationHarness();const run=h[action]();h.select('b');h.select('a');h.pending[0].resolve({id:'new',name:'New'});await run;
  assert.equal(h.state().selectedProfileId,'a');assert.equal(h.state().profileDetail.note,'new draft');
 });
 test(`${action} selects new profile if user has not navigated`,async()=>{
  const h=creationHarness();const run=h[action]();h.pending[0].resolve({id:'new',name:'New'});await run;
  assert.equal(h.state().selectedProfileId,'new');assert.equal(h.state().busy,false);
 });
}
test('delete hold cancels when selection changes instead of deleting the new target',()=>{
 let frame;let removed=0;let clock=0;
 const code=`let busy=false,selectedProfileId='a',profileDetail={id:'a',revision:'seen'},deleteHoldTargetId=null,deleteHoldProgress=0,deleteHoldTimer=null,deleteHoldStartedAt=0,deleteHoldDurationMs=1200;
 ${body('startDeleteHold')} ${body('cancelDeleteHold')}
 return {startDeleteHold,select(id){selectedProfileId=id;},target:()=>deleteHoldTargetId};`;
 const h=new Function('window','performance','deleteSelectedProfile',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)({requestAnimationFrame:fn=>{frame=fn;return 1;},cancelAnimationFrame:()=>{}},{now:()=>clock},()=>removed++);
 h.startDeleteHold('a');h.select('b');clock=1300;frame();assert.equal(removed,0);assert.equal(h.target(),null);
});

test('profile list refresh updates saved default independently of editor and launch target',async()=>{
 const code=`let profiles=[{id:'a'},{id:'b'}],profilesReady=true,selectedProfileId='a',persistedSelectedProfileId='a',launchTargetId='a',profileDetail={id:'a',note:'unsaved'},errorMessage='';
 const pickInitialLaunchTargetId=id=>id;const refreshProfileDetail=async()=>{throw Error('must retain editor');};
 let profileListRequest=0;const retainProfileOrder=list=>list;${body('refreshProfiles')}
 return {refreshProfiles,state:()=>({persistedSelectedProfileId,selectedProfileId,launchTargetId,profileDetail})};`;
 const h=new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(async cmd=>cmd==='list_profiles'?[{id:'a'},{id:'b'}]:'b');
 await h.refreshProfiles(false);assert.equal(h.state().persistedSelectedProfileId,'b');assert.equal(h.state().selectedProfileId,'a');assert.equal(h.state().launchTargetId,'a');assert.equal(h.state().profileDetail.note,'unsaved');
});

function listRaceHarness(){
 const pending=[];const invoke=(cmd)=>new Promise((resolve,reject)=>pending.push({cmd,resolve,reject}));
 const code=`let profileListRequest=0,profiles=[{id:'a'}],profilesReady=true,selectedProfileId='a',persistedSelectedProfileId='a',launchTargetId='a',profileDetail={id:'a',note:'draft'},errorMessage='';
 const pickInitialLaunchTargetId=id=>id;const refreshProfileDetail=async()=>{throw Error('unexpected detail refresh');};
 const retainProfileOrder=list=>list;${body('refreshProfiles')}
 return {refreshProfiles,changeList(){profiles=[...profiles,{id:'new'}];},changeDefault(){persistedSelectedProfileId='new';},state:()=>({profiles,persistedSelectedProfileId,selectedProfileId,profileDetail,errorMessage})};`;
 return {...new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(invoke),pending};
}
test('older list success and failure cannot supersede the newest refresh',async()=>{
 for(const fail of [false,true]){
  const h=listRaceHarness();const old=h.refreshProfiles(false),recent=h.refreshProfiles(false);
  h.pending[2].resolve([{id:'a',name:'latest'}]);h.pending[3].resolve('b');await recent;
  if(fail)h.pending[0].reject(Error('obsolete'));else h.pending[0].resolve([{id:'a',name:'old'}]);
  h.pending[1].resolve('a');await old;
  assert.equal(h.state().profiles[0].name,'latest');assert.equal(h.state().persistedSelectedProfileId,'b');assert.equal(h.state().errorMessage,'');
 }
});
for(const mutation of ['changeList','changeDefault'])test(`pending list does not overwrite ${mutation}`,async()=>{
 const h=listRaceHarness();const run=h.refreshProfiles(false);h[mutation]();
 h.pending[0].resolve([{id:'a'}]);h.pending[1].resolve('a');await run;
 if(mutation==='changeList')assert.ok(h.state().profiles.some(p=>p.id==='new'));
 else assert.equal(h.state().persistedSelectedProfileId,'new');
 assert.equal(h.state().profileDetail.note,'draft');
});

test('current list failure remains visible and preserves the existing editor',async()=>{
 const h=listRaceHarness();const run=h.refreshProfiles(false);
 h.pending[0].reject(Error('offline'));h.pending[1].resolve('a');await run;
 assert.match(h.state().errorMessage,/offline/);assert.equal(h.state().selectedProfileId,'a');
 assert.equal(h.state().profileDetail.note,'draft');assert.equal(h.state().profiles.length,1);
});

test('same-profile legacy responses are ordered and leave the newest summary',async()=>{
 for(const batch of [false,true]){
  const h=harness();
  const old=batch?h.updateProfileFields({note:'old'}):h.updateProfileField('note','old');
  const latest=batch?h.updateProfileFields({note:'latest'}):h.updateProfileField('note','latest');
  assert.equal(h.pending.length,1);
  h.pending[0].resolve({id:'a',name:'Alpha',note:'old',runtime:{}});await old;
  h.pending[1].resolve({id:'a',name:'Alpha',note:'latest',runtime:{}});await latest;
  assert.equal(h.state().profileDetail.note,'latest');
  assert.equal(h.state().profiles.find(p=>p.id==='a').note,'latest');
 }
});


test('summary response ordering is independent for each profile',async()=>{
 const h=harness();
 const a=h.updateProfileField('note','Alpha saved','a');
 const b=h.updateProfileFields({note:'Beta saved'},'b');
 h.pending[1].resolve({id:'b',name:'Beta',note:'Beta saved',runtime:{}});await b;
 h.pending[0].resolve({id:'a',name:'Alpha',note:'Alpha saved',runtime:{}});await a;
 assert.equal(h.state().profiles.find(p=>p.id==='a').note,'Alpha saved');
 assert.equal(h.state().profiles.find(p=>p.id==='b').note,'Beta saved');
});

test('newer failed queued save retains the last successful summary and failed input',async()=>{
 const h=harness();
 const old=h.updateProfileField('note','saved');
 const latest=h.updateProfileFields({note:'unsaved'});
 assert.equal(h.pending.length,1);
 h.pending[0].resolve({id:'a',name:'Alpha',note:'saved',runtime:{}});await old;
 h.pending[1].reject(Error('write denied'));await latest;
 assert.equal(h.state().profiles.find(p=>p.id==='a').note,'saved');
 assert.equal(h.state().profileWrites[0].fields.note,'unsaved');
});


test('revision-aware single and batch handlers serialize and carry accepted revision',async()=>{
 const h=harness();const load=h.refreshProfileDetail('a');
 h.pending[0].resolve({id:'a',name:'Alpha',revision:'r1',runtime:{}});await load;
 const first=h.updateProfileField('note','one');const second=h.updateProfileFields({group:'two'});
 assert.equal(h.pending.length,2);assert.equal(h.pending[1].args.expectedRevision,'r1');
 h.pending[1].resolve({id:'a',name:'Alpha',note:'one',revision:'r2',runtime:{}});await first;await Promise.resolve();
 assert.equal(h.pending.length,3);assert.equal(h.pending[2].args.expectedRevision,'r2');
 h.pending[2].resolve({id:'a',name:'Alpha',note:'one',group:'two',revision:'r3',runtime:{}});await second;
 assert.equal(h.state().profileWrites.length,0);assert.equal(h.state().profileDetail.revision,'r3');
});

test('conflicted input retains original revision after reloading newer data',async()=>{
 const h=harness();const load=h.refreshProfileDetail('a');
 h.pending[0].resolve({id:'a',name:'Alpha',revision:'r1',runtime:{}});await load;
 const save=h.updateProfileField('note','retained');h.pending[1].reject(Error('profile_conflict'));await save;
 const reload=h.refreshProfileDetail('a');h.pending[2].resolve({id:'a',name:'Alpha',note:'external',revision:'r9',runtime:{}});await reload;
 const write=h.state().profileWrites[0];assert.equal(write.expectedRevision,'r1');assert.equal(write.fields.note,'retained');
 const retry=h.retryProfileWrite(write);assert.equal(h.pending[3].args.expectedRevision,'r1');
 h.pending[3].reject(Error('profile_conflict'));await retry;
 assert.equal(h.state().profileWrites[0].fields.note,'retained');
});

for(const outcome of ['success','failure']){
 test(`Capturer profile A-B-A rejects earlier A ${outcome}`,async()=>{
  const pending=[];const invoke=()=>new Promise((resolve,reject)=>pending.push({resolve,reject}));
  const code=`let selectedCapturerProfileId='a',selectedCapturerProfileDetail=null,capturerProfileDetailRequest=0;
   ${body('refreshSelectedCapturerProfileDetail')}
   return {read:refreshSelectedCapturerProfileDetail,select(id){selectedCapturerProfileId=id},value:()=>selectedCapturerProfileDetail};`;
  const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const h=new Function('invoke',js)(invoke);
  const old=h.read('a');h.select('b');const b=h.read('b');h.select('a');const current=h.read('a');
  pending[2].resolve({id:'a',revision:'latest'});await current;
  if(outcome==='success')pending[0].resolve({id:'a',revision:'stale'});else pending[0].reject(Error('offline'));
  await old;pending[1].resolve({id:'b'});await b;
  assert.deepEqual(h.value(),{id:'a',revision:'latest'});
 });
}

for (const batch of [false,true]) {
 test(`resolved delivery clears only its own confirmed warning (${batch})`,async()=>{
  const h=harness();
  const save=(id,targets)=>{
   const p=batch?h.updateProfileFields({note:'saved'},id):h.updateProfileField('note','saved',id);
   h.pending.at(-1).resolve({id,name:id,note:'saved',runtime:{},syncReport:{targets,discoveryError:null}});return p;
  };
  await save('a',[{capturerId:7,accepted:false,error:'HTTP 409',statusCode:409}]);
  const warning=h.state().errorMessage;assert.match(warning,/保存済み/);
  await save('b',[{capturerId:8,accepted:true,error:null}]);
  assert.equal(h.state().errorMessage,warning,'other profile cannot dismiss warning');
  await save('a',[]);assert.equal(h.state().errorMessage,warning,'missing recipient is not acknowledgement');
  await save('a',[{capturerId:7,accepted:true,error:null,statusCode:200}]);
  assert.equal(h.state().errorMessage,'');
 });
}


test('restore list response cannot overwrite a newer saved summary',async()=>{
 const pending=[]; const h=restorationHarness((cmd)=>new Promise(resolve=>pending.push({cmd,resolve})));
 const operation=h.restoreArchivedProfile('archive-a');
 pending[0].resolve({selectedProfileId:'b'}); await Promise.resolve();
 h.editSummary(); pending[1].resolve([{id:'b',note:'old'},{id:'a'}]);
 await assert.rejects(operation,/profiles.archive.refresh_failed/);
 assert.equal(h.state().profiles[0].note,'newer saved note');
});

test('restore result cannot overwrite a newer saved default',async()=>{
 const pending=[]; const h=restorationHarness((cmd)=>new Promise(resolve=>pending.push({cmd,resolve})));
 const operation=h.restoreArchivedProfile('archive-a'); h.saveDefault();
 pending[0].resolve({selectedProfileId:'b'}); await Promise.resolve();
 pending[1].resolve([{id:'b'},{id:'a'},{id:'c'}]); await operation;
 assert.equal(h.state().persistedSelectedProfileId,'c');
});


test('late name save for A refreshes summaries without replacing an in-flight B edit',async()=>{
 const h=harness(false,true);
 const a=h.updateProfileField('name','Alpha renamed');
 const select=h.selectProfile('b');h.pending[1].resolve({id:'b',name:'Beta',note:'old',revision:'b1'});await select;
 const b=h.updateProfileField('note','new B note');
 h.pending[0].resolve({id:'a',name:'Alpha renamed',note:'',runtime:{},revision:'a2'});
 await new Promise(resolve=>setImmediate(resolve));
 assert.equal(h.pending[3].cmd,'list_profiles');assert.equal(h.pending[4].cmd,'selected_profile_id');
 h.pending[3].resolve([{id:'a',name:'Alpha renamed'},{id:'b',name:'Beta',note:'old'}]);h.pending[4].resolve('a');
 await new Promise(resolve=>setImmediate(resolve));
 // Release an unwanted old read so the failing baseline cannot leave work hanging.
 const staleRead=h.pending[5];
 if(staleRead){assert.equal(staleRead.cmd,'get_profile_detail');staleRead.resolve({id:'b',name:'Beta',note:'old',revision:'b1'});}
 await a;
 h.pending[2].resolve({id:'b',name:'Beta',note:'new B note',runtime:{},revision:'b2'});await b;
 assert.equal(h.state().profileDetail.note,'new B note');
 assert.equal(staleRead,undefined,'summary refresh must not reload the current editor');
});

for (const otherFailure of [false,true]) for (const selected of ['a','b']) for (const action of ['finishCapturerProfileSave','clearAllCapturerNeutralCalibrations','calibrateAllCapturersNeutral']) {
 test(`${action} refreshes calibrated A and preserves unrelated B: selected=${selected}, otherFailure=${otherFailure}`,async()=>{
  const detail={id:'a',name:'Alpha',revision:'a2',runtime:{},syncReport:{targets:[],discoveryError:null}};
  const code=`let profileDetailRequest=0;let selectedProfileId='${selected}',profileDetail={id:'${selected}',note:'pending B input'},selectedCapturerProfileId='a',selectedCapturerProfileDetail=null,capturerProfileDetailRequest=0;
  let profileListRequest=0,profiles=[{id:'a'},{id:'b'}],persistedSelectedProfileId='a',profilesReady=true,launchTargetId='a',busy=false,calibratingCapturerId=null,actionCountdownOverlay=null,message='',errorMessage='',$locale='ja-JP',detailReads=0;
  const profileSaveQueue={captureObserver(){return ()=>true;},observe(){}};const retainProfileOrder=list=>list;const pickInitialLaunchTargetId=id=>id;
  const refreshProfileDetail=async()=>{detailReads++;profileDetail={id:'b',note:'old saved B'};};
  const refreshRuntimeStatuses=async()=>{};const runningCapturers=()=>[{id:17,profileId:'a'}];
  const runActionCountdown=async()=>{};const calibrationPoseDisplayLabel=()=> 'Head';const calibrationOverlay=()=>({});const invokeCapturerNeutralCalibration=async()=>detail;
  ${body('refreshProfiles')} ${body(action)}
  return {run:()=>${action==='finishCapturerProfileSave' ? "finishCapturerProfileSave(detail,'saved')" : action+'()'},state:()=>({profileDetail,detailReads,busy,errorMessage,selectedCapturerProfileDetail})};`;
  const invoke=async cmd=>cmd==='list_profiles'?[{id:'a'},{id:'b'}]:cmd==='selected_profile_id'?'a':detail;
  const feedback=new ProfileSyncFeedback();
  if(otherFailure)feedback.update({id:'other',name:'Other',syncReport:{targets:[{capturerId:99,accepted:false,error:'offline'}],discoveryError:null}},'ja-JP','');
  const h=new Function('profileSyncFeedback','invoke','detail','profileSyncWarning','savedProfileRefreshWarning',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(feedback,invoke,detail,profileSyncWarning,savedProfileRefreshWarning);
  await h.run();
  if(selected==='b')assert.equal(h.state().profileDetail.note,'pending B input');
  else assert.equal(h.state().profileDetail.revision,'a2');
  assert.equal(h.state().detailReads,0);
  assert.equal(h.state().busy,false);
  if(otherFailure)assert.match(h.state().errorMessage,/Capturer #99/);else assert.equal(h.state().errorMessage,'');
  if(action==='finishCapturerProfileSave')assert.equal(h.state().selectedCapturerProfileDetail.revision,'a2');
 });
}

for (const readable of [true,false]) test(`capturer restart validates profile before stopping (readable=${readable})`,async()=>{
 const calls=[];
 const code=`const $_=(key,options)=>key+JSON.stringify(options??{});let restartingCapturerId=null,busy=false,message='',errorMessage='',selectedCapturerId=null;const refreshCapturers=async()=>{};
 ${body('restartCapturer')}
 return {restartCapturer,state:()=>({busy,message,errorMessage})};`;
 const h=new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(async(cmd,args)=>{
  calls.push({cmd,args});
  if(cmd==='get_profile_detail') {if(!readable)throw Error('profile unreadable');return {id:'original'};}
  if(cmd==='launch_capturer')return {id:8};
 });
 await h.restartCapturer({id:7,profileId:'original'});
 assert.deepEqual(calls[0],{cmd:'get_profile_detail',args:{profileId:'original'}});
 if(readable)assert.deepEqual(calls.slice(1),[{cmd:'stop_capturer',args:{id:7}},{cmd:'launch_capturer',args:{profileId:'original',allowNonLoopback:false}}]);
 else {assert.equal(calls.length,1);assert.match(h.state().errorMessage,/profile unreadable/);}
 assert.equal(h.state().busy,false);
});

for (const move of [false,true]) test('capturer restart retains newer selection: '+move,async()=>{
 let finish;const gate=new Promise(resolve=>finish=resolve);
 const code=`let restartingCapturerId=null,busy=false,message='',errorMessage='',selectedCapturerId=7;const hasTauriRuntime=()=>true,$_=(s)=>s;const refreshCapturers=async()=>{};
 ${body('restartCapturer')}
 return {run:()=>restartCapturer({id:7,profileId:'a'}),move:()=>{selectedCapturerId=99},selected:()=>selectedCapturerId};`;
 const h=new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(async(cmd)=>{
  if(cmd==='get_profile_detail')return {id:'a'};
  if(cmd==='launch_capturer'){await gate;return {id:8,name:'A'};}
 });
 const task=h.run();await new Promise(resolve=>setImmediate(resolve));if(move)h.move();finish();await task;
 assert.equal(h.selected(),move?99:8);
});

for (const failure of ['none','launch','refresh','both']) test(`group launch retains completed work and original error: ${failure}`,async()=>{
 const calls=[];
 const code=`let busy=false,message='',errorMessage='',selectedCapturerId=7,launchGroupName='Group A',launchProfileId='a',launchGroupProfiles=[{id:'a',name:'A'},{id:'b',name:'B'}];
 const $_=(key,options)=>key+JSON.stringify(options??{});
 ${body('launchCapturer')}
 return {run:launchCapturer,change:()=>{launchGroupProfiles=[{id:'c',name:'C'}];selectedCapturerId=99;},state:()=>({busy,message,errorMessage,selectedCapturerId})};`;
 let h;
 h=new Function('invoke','refreshCapturers',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(async(cmd,args)=>{
  calls.push(args.profileId);
  if(args.profileId==='a'){h.change();return {id:10};}
  if(failure==='launch'||failure==='both')throw Error('launch-original-error');
  return {id:11};
 },async()=>{if(failure==='refresh'||failure==='both')throw Error('refresh-secondary-error');});
 const launched=await h.run();
 assert.equal(launched,failure==='none'||failure==='refresh');
 assert.deepEqual(calls,['a','b']);assert.equal(h.state().busy,false);
 assert.equal(h.state().selectedCapturerId,99);
 if(failure==='launch'||failure==='both'){assert.match(h.state().errorMessage,/launch-original-error/);assert.match(h.state().message,/partial_launch/);assert.match(h.state().message,/10/);}
 else {assert.match(h.state().message,/launched_group/);assert.match(h.state().message,/"count":2/);}
 if(failure==='refresh'||failure==='both')assert.match(h.state().errorMessage,/refresh-secondary-error/);
});

test('startup does not consume its attempt while another operation is busy',async()=>{
 const code=`let busy=true,appSettingsReady=true,startupAutoLaunchAttempted=false,activeTab='settings',launches=0;
 const appSettings={autoLaunchSelectedOnStartup:true},launchTargetId='a',profiles=[],isValidLaunchTarget=()=>true,launchTargetSetting={id:'a'};
 const visibleRenderers=[],liveCapturers=()=>[],setActiveTab=tab=>{activeTab=tab},launchCapturer=async()=>{if(!busy)launches++;};
 ${body('maybeAutoLaunchSelectedOnStartup')}
 return {run:maybeAutoLaunchSelectedOnStartup,idle:()=>{busy=false},state:()=>({launches,startupAutoLaunchAttempted,activeTab})};`;
 const h=new Function(ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)();
 await h.run();assert.deepEqual(h.state(),{launches:0,startupAutoLaunchAttempted:false,activeTab:'settings'});
 h.idle();await h.run();await h.run();assert.deepEqual(h.state(),{launches:1,startupAutoLaunchAttempted:true,activeTab:'capturers'});
});

for(const state of ['saving','failed']) for(const action of ['duplicateSelectedProfile','deleteSelectedProfile']) {
 test(`${action} retains source ${state} edits and sends no mutation`,async()=>{
  const h=action==='duplicateSelectedProfile'?creationHarness():deletionHarness();
  h.pendingEdit('a',state);const task=h[action]();
  if(h.pending.length)h.pending[0].reject(Error('unexpected mutation'));
  await task;assert.equal(h.pending.length,0);assert.equal(h.state().selectedProfileId,'a');
  assert.equal(h.state().profileWrites.length,1);assert.match(h.state().errorMessage,/pending_edits/);
 });
}
test('duplicate saved source is not blocked by another profile failed edit',async()=>{
 const h=creationHarness();h.pendingEdit('b','failed');const task=h.duplicateSelectedProfile();
 assert.equal(h.pending.length,1);h.pending[0].resolve({id:'copy',name:'Copy'});await task;
 assert.equal(h.state().profileWrites[0].profileId,'b');
});

test('delete saved source retains another profile failed edit',async()=>{
 const h=deletionHarness();h.pendingEdit('b','failed');const task=h.deleteSelectedProfile();
 assert.equal(h.pending[0].args.profileId,'a');h.pending[0].resolve();await Promise.resolve();
 h.pending[1].resolve([{id:'b'}]);await task;
 assert.equal(h.state().profileWrites[0].profileId,'b');assert.equal(h.state().selectedProfileId,'b');
});

function webcamDiscoveryHarness() {
 const pending=[];
 const invoke=(command,args)=>new Promise((resolve,reject)=>pending.push({command,args,resolve,reject}));
 const code=`let profileDetail=null,isMediaPipeEngine=true,mediaPipeSourceType='webcam-directshow';
 let webcamDevices=[],webcamDevicesLoading=false,webcamDevicesError='',webcamFormats=[],webcamFormatsLoading=false,webcamFormatsError='',webcamDevicesRequest=0,webcamFormatsRequest=0;
 ${['resolveEngineType','normalizeEngineType','resolveMediaPipeSourceType','currentWebcamBackend','refreshWebcamDevices','refreshWebcamFormats'].map(body).join('\n')}
 return {select(detail){profileDetail=detail;isMediaPipeEngine=resolveEngineType(detail)==='mediapipe-native';mediaPipeSourceType=resolveMediaPipeSourceType(detail);},devices:refreshWebcamDevices,formats:refreshWebcamFormats,state:()=>({webcamDevices,webcamFormats,webcamDevicesLoading,webcamFormatsLoading,webcamDevicesError,webcamFormatsError})};`;
 return {...new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(invoke),pending};
}
for(const engine of [null,'vmc','ifacialmocap']) {
 test(`camera discovery never queries hardware for ${engine??'no selected profile'}`,async()=>{
  const h=webcamDiscoveryHarness();h.select(engine?{runtime:{engine},pipeline:{input:'webcam-directshow'}}:null);
  const devices=h.devices();const formats=h.formats('old camera');
  assert.equal(h.pending.length,0);
  await Promise.all([devices,formats]);
  assert.deepEqual(h.state().webcamDevices,[]);assert.deepEqual(h.state().webcamFormats,[]);
 });
}
test('camera discovery remains functional and ignores results after switching to UDP input',async()=>{
 const h=webcamDiscoveryHarness();
 h.select({runtime:{engine:'mediapipe-native'},pipeline:{input:'webcam-mediafoundation'}});
 const devices=h.devices();const formats=h.formats('camera1');
 assert.deepEqual(h.pending.map(p=>p.command),['enumerate_webcams','enumerate_webcam_formats']);
 assert.equal(h.pending[0].args.backend,'mediafoundation');
 h.select({runtime:{engine:'vmc'},pipeline:{input:null}});
 const cleared=[h.devices(),h.formats('camera1')];
 assert.equal(h.pending.length,2);await Promise.all(cleared);
 h.pending[0].resolve([{id:'camera1'}]);h.pending[1].resolve([{width:1920}]);
 await Promise.all([devices,formats]);
 assert.deepEqual(h.state().webcamDevices,[]);assert.deepEqual(h.state().webcamFormats,[]);
 assert.equal(h.state().webcamDevicesLoading,false);assert.equal(h.state().webcamFormatsLoading,false);
 h.select({runtime:{engine:'mediapipe-native'},pipeline:{input:'webcam-directshow'}});
 const current=h.devices();assert.equal(h.pending[2].args.backend,'directshow');h.pending[2].resolve([{id:'current'}]);await current;
 assert.deepEqual(h.state().webcamDevices,[{id:'current'}]);
});
for (const batch of [false, true]) {
 test(`queued ${batch ? 'batch' : 'field'} sends the retained nested snapshot`, async () => {
  const h = harness();
  const first = h.updateProfileField('note', 'first');
  const value = { parts: [1, 2] };
  const queued = batch ? h.updateProfileFields({ value }) : h.updateProfileField('value', value);
  value.parts.push(3);
  h.pending[0].resolve({ id: 'a', runtime: {} });
  await first;
  await Promise.resolve();
  const sent = batch ? h.pending[1].args.updates[0].value : h.pending[1].args.value;
  const retained = h.state().profileWrites.find(write => 'value' in write.fields).fields.value;
  h.pending[1].reject(Error('disk unavailable'));
  await queued;
  assert.deepEqual(sent, { parts: [1, 2] });
  assert.deepEqual(sent, retained);
 });
}

for (const operation of ['browseProfileIcon','browseInputFile']) {
 for (const outcome of ['cancel','error']) test(`${operation} ${outcome} after selection switch never writes either profile`,async()=>{
  const h=harness();const pending=h[operation]();
  assert.equal(h.pending[0].cmd,'pick_file_path');
  const select=h.selectProfile('b');h.pending[1].resolve({id:'b',note:'untouched'});await select;
  const before=structuredClone(h.state().profileDetail);
  if(outcome==='cancel')h.pending[0].resolve(null);else h.pending[0].reject(Error('dialog failed'));
  await pending;
  assert.equal(h.pending.length,2,'No save or metadata probe after cancel/error');
  assert.deepEqual(h.state().profileDetail,before);
  assert.equal(h.state().selectedProfileId,'b');
  assert.equal(h.state().profileWrites.length,0);
  if(outcome==='error')assert.match(h.state().errorMessage,/dialog failed/);
 });
}

for (const scenario of ['success','failed','changed-profile','changed-page']) test(`quick launch respects result and navigation: ${scenario}`,async()=>{
 let finish;const gate=new Promise(resolve=>finish=resolve);
 const code=`let busy=false,selectedProfileId='a',profileDetailRequest=1,activeTab='profiles',launchTargetId='b';
 const appSettings={jumpToCapturersOnQuickLaunch:true};
 ${body('quickLaunchSelectedProfile')}
 return {run:quickLaunchSelectedProfile,move:()=>{if(scenario==='changed-profile'){selectedProfileId='b';profileDetailRequest++;}if(scenario==='changed-page')activeTab='logs';},state:()=>({activeTab,launchTargetId})};`;
 const h=new Function('launchCapturer','scenario',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(async()=>{await gate;return scenario!=='failed';},scenario);
 const task=h.run();h.move();finish();await task;
 assert.equal(h.state().launchTargetId,'a');
 assert.equal(h.state().activeTab,scenario==='success'?'capturers':scenario==='changed-page'?'logs':'profiles');
});


test('older video probe cannot overwrite a newer video path on the same profile', async()=>{
 const h=harness();const older=h.updateVideoInputPath('old.mp4');const newer=h.updateVideoInputPath('new.mp4');
 h.pending[1].resolve({fpsRounded:30,width:1280,height:720,fps:30});await Promise.resolve();
 assert.equal(h.pending[2].args.updates.find(x=>x.field==='pipeline_components.input_path').value,'new.mp4');
 h.pending[2].resolve({id:'a',runtime:{}});await newer;
 h.pending[0].resolve({fpsRounded:60,width:1920,height:1080,fps:60});await Promise.resolve();
 assert.equal(h.pending.length,3,'Late old metadata must not enqueue another save');await older;
});


test('clearing a video path rejects late metadata failures without restoring the path', async()=>{
 const h=harness();const probe=h.updateVideoInputPath('old.mp4');const clear=h.updateVideoInputPath(null);
 h.pending[1].resolve({id:'a',runtime:{}});await clear;
 const before=h.state().errorMessage;h.pending[0].reject(Error('old probe failed'));await probe;
 assert.equal(h.pending.length,2);assert.equal(h.state().errorMessage,before);
});

test('changing input type invalidates outstanding video metadata', async()=>{
 const h=harness();const probe=h.updateVideoInputPath('old.mp4');const change=h.updateProfileField('pipeline_components.input','file-image');
 h.pending[1].resolve({id:'a',runtime:{}});await change;
 h.pending[0].resolve({fpsRounded:60,width:1920,height:1080,fps:60});await probe;
 assert.equal(h.pending.length,2);
});

test('video probes on different profiles remain independent', async()=>{
 const h=harness();const a=h.updateVideoInputPath('a.mp4','a');const b=h.updateVideoInputPath('b.mp4','b');
 h.pending[1].resolve({fpsRounded:30,width:640,height:480,fps:30});await Promise.resolve();
 assert.equal(h.pending[2].args.profileId,'b');h.pending[2].resolve({id:'b',runtime:{}});await b;
 h.pending[0].resolve({fpsRounded:60,width:1280,height:720,fps:60});await Promise.resolve();
 assert.equal(h.pending[3].args.profileId,'a');h.pending[3].resolve({id:'a',runtime:{}});await a;
});
