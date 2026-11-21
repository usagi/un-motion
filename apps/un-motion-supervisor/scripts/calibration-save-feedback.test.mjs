import {ProfileSaveQueue} from '../src/lib/profileSaveQueue.ts';
import assert from 'node:assert/strict';
import test from 'node:test';
import {readFile} from 'node:fs/promises';
import ts from 'typescript';
import {ProfileSyncFeedback,profileSyncWarning,savedProfileRefreshWarning} from '../src/lib/profileSync.ts';
const source=await readFile(new URL('../src/App.svelte',import.meta.url),'utf8');
function body(name){
 const start=source.indexOf(`\tasync function ${name}(`);
 return start<0?'':source.slice(start,source.indexOf('\n\t}',start)+3);
}
for(const name of ['calibrateCapturerNeutral','clearCapturerNeutralCalibration','buildCapturerFacePoseModel','clearCapturerFacePoseModel']){
 test(`${name} retains saved/delivery result when later refresh fails`,async()=>{
  const detail={id:'a',name:'Alpha',revision:'saved-revision',runtime:{},syncReport:{discoveryError:null,targets:[{capturerId:7,accepted:false,error:'HTTP 409'}]}};
  const code=`let profileDetailRequest=0,capturerProfileDetailRequest=0;let busy=false,calibratingCapturerId=null,message='',errorMessage='',actionCountdownOverlay=null;
   let selectedProfileId='b',profileDetail={id:'b',name:'Beta'},selectedCapturerProfileId='a',selectedCapturerProfileDetail={id:"a",revision:"read-revision"};
   let appSettings={},$locale='ja-JP';const profileSaveQueue={captureObserver(){return ()=>true;},observe(){},revision(){return "read-revision"},run(id,revision,save){return save(revision)}};
   async function invoke(){return detail;}async function invokeCapturerNeutralCalibration(){return detail;}
   async function runCalibrationCountdown(){}async function runActionCountdown(){}function calibrationPoseDisplayLabel(){return 'U';}function faceModelOverlay(){return {};}
   async function refreshProfiles(){}async function refreshRuntimeStatuses(){throw Error('refresh offline');}
   ${body('finishCapturerProfileSave')} ${body(name)}
   await ${name}({id:7,profileId:'a'},'U');return {message,errorMessage,profileDetail,selectedCapturerProfileDetail};`;
  const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const run=new Function('profileSyncFeedback','detail','profileSyncWarning','savedProfileRefreshWarning',`return (async()=>{${js}})();`);
  const result=await run(new ProfileSyncFeedback(),detail,profileSyncWarning,savedProfileRefreshWarning);
  assert.match(result.errorMessage,/保存済み/);assert.match(result.errorMessage,/HTTP 409/);assert.match(result.errorMessage,/refresh offline/);
  assert.ok(result.message);assert.equal(result.profileDetail.id,'b');assert.equal(result.selectedCapturerProfileDetail.id,'a');
 });
}

for (const revision of ['displayed-revision', undefined]) {
 test(`clear correction protects displayed revision ${revision}`, async()=>{
  const code=`let profileDetailRequest=0,capturerProfileDetailRequest=0;let busy=false,calibratingCapturerId=null,message='',errorMessage='';
   let selectedCapturerProfileDetail={id:'a',revision},profileDetail={id:'b',revision:'other'};
   const calls=[];const profileSaveQueue={captureObserver(){return ()=>true;},revision(){return 'cached'},run(id,base,save){return save(base)}};
   async function invoke(command,args){calls.push({command,args});throw Error('profile_conflict');}
   async function finishCapturerProfileSave(){throw Error('must not accept rejected save');}
   ${body('clearCapturerFacePoseModel')}
   await clearCapturerFacePoseModel({id:7,profileId:'a'});return {calls,errorMessage,busy};`;
  const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const result=await new Function('revision',`return (async()=>{${js}})();`)(revision);
  assert.equal(result.busy,false);
  if(revision){assert.equal(result.calls.length,1);assert.equal(result.calls[0].args.expectedRevision,revision);assert.equal(result.calls[0].args.profileId,'a');assert.match(result.errorMessage,/profile_conflict/);}
  else {assert.equal(result.calls.length,0);assert.match(result.errorMessage,/再読み込み/);}
 });
}

test('selected-profile correction preserves build failure when temporary cleanup also fails',async()=>{
 const code=`let profileDetailRequest=0,capturerProfileDetailRequest=0;let busy=false,calibratingCapturerId=null,message='',errorMessage='',actionCountdownOverlay=null;
 const profileSaveQueue={captureObserver(){return ()=>true;}};
 let profileDetail={id:'a',name:'Alpha'},capturerListGeneration=0,capturers=[],selectedCapturerId=null,appSettings={};
 async function refreshCapturers(){capturerListGeneration++;}
 async function runActionCountdown(){}function faceModelOverlay(){return {};}
 async function invoke(command){if(command==='launch_capturer')return {id:7};if(command==='build_capturer_face_pose_model')throw Error('measurement failed');if(command==='stop_capturer')throw Error('cleanup offline');}
 ${body('buildSelectedProfileFacePoseModel')}
 await buildSelectedProfileFacePoseModel();return {errorMessage,message,busy};`;
 const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
 const result=await new Function(`return (async()=>{${js}})();`)();
 assert.match(result.errorMessage,/measurement failed/);assert.match(result.errorMessage,/cleanup offline/);assert.doesNotMatch(result.errorMessage,/saved/);assert.equal(result.busy,false);
});

for (const language of ['ja-JP','en-US']) {
 test(`sync target rejection stays distinct from unknown delivery (${language})`,()=>{
  const report={targets:[{capturerId:7,accepted:false,error:'HTTP 409',statusCode:409},
   {capturerId:8,accepted:false,error:'timeout',statusCode:null},
   {capturerId:9,accepted:true,error:null,statusCode:200}],discoveryError:null};
  const message=profileSyncWarning({id:'a',name:'Alpha',syncReport:report},language);
  assert.match(message,language==='ja-JP'?/保存済み/:/were saved/);
  assert.match(message,language==='ja-JP'?/適用対象が一致しない/:/target does not match/);
  assert.match(message,/Capturer #7/);assert.match(message,/Capturer #8: timeout/);
  assert.doesNotMatch(message,/Capturer #9/);
 });
}

for(const change of ['edit']) {
 test(`late calibration result preserves same-profile ${change}`,async()=>{
  const code=`let busy=false,calibratingCapturerId=null,message='',errorMessage='',actionCountdownOverlay=null;
   let profileDetailRequest=0,capturerProfileDetailRequest=0,selectedProfileId='a',profileDetail={id:'a',note:'initial'},selectedCapturerProfileId='a',selectedCapturerProfileDetail={id:'a',note:'initial'};
   let $locale='ja-JP';const profileSaveQueue=new ProfileSaveQueue();profileSaveQueue.observe('a','initial');let resolve;
   const invoke=()=>new Promise(r=>resolve=r);const refreshProfiles=async()=>{};const refreshRuntimeStatuses=async()=>{};
   ${body('finishCapturerProfileSave')} ${body('clearCapturerNeutralCalibration')}
   return {run:()=>clearCapturerNeutralCalibration({id:7,profileId:'a'}),complete:()=>resolve({id:'a',note:'stale calibration response',revision:'calibration',runtime:{}}),change(){profileSaveQueue.observe('a','newer');profileDetailRequest++;capturerProfileDetailRequest++;profileDetail={id:'a',note:'newer editor'};selectedCapturerProfileDetail={id:'a',note:'newer runtime detail'};},state:()=>({profileDetail,selectedCapturerProfileDetail,cachedRevision:profileSaveQueue.revision('a')})};`;
  const h=new Function('profileSyncFeedback','ProfileSaveQueue','profileSyncWarning','savedProfileRefreshWarning',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(new ProfileSyncFeedback(),ProfileSaveQueue,profileSyncWarning,savedProfileRefreshWarning);
  const run=h.run();h.change();h.complete();await run;
  assert.equal(h.state().profileDetail.note,'newer editor');
  assert.equal(h.state().selectedCapturerProfileDetail.note,'newer runtime detail');
  assert.equal(h.state().cachedRevision,'newer');
 });
}

for (const refreshFails of [false,true]) {
 test(`calibration save retains another profile delivery failure (refreshFails=${refreshFails})`,async()=>{
  const feedback=new ProfileSyncFeedback();
  feedback.update({id:'other',name:'Other',syncReport:{discoveryError:null,targets:[{capturerId:99,accepted:false,error:'offline'}]}},'ja-JP','');
  const code=`let profileDetailRequest=0,capturerProfileDetailRequest=0,selectedProfileId='a',selectedCapturerProfileId='a',profileDetail=null,selectedCapturerProfileDetail=null,message='',errorMessage='';
   const $locale='ja-JP';const profileSaveQueue={captureObserver(){return ()=>{};}};
   const refreshProfiles=async()=>{};const refreshRuntimeStatuses=async()=>{if(refreshFails)throw Error('refresh offline');};
   ${body('finishCapturerProfileSave')}
   await finishCapturerProfileSave({id:'a',name:'A',syncReport:{targets:[],discoveryError:null}},'saved');
   return {errorMessage,message};`;
  const js=ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText;
  const result=await new Function('profileSyncFeedback','profileSyncWarning','savedProfileRefreshWarning','refreshFails',`return (async()=>{${js}})();`)(feedback,profileSyncWarning,savedProfileRefreshWarning,refreshFails);
  assert.match(result.errorMessage,/Capturer #99/);
  assert.equal(result.message,'saved');
  if(refreshFails)assert.match(result.errorMessage,/refresh offline/);
 });
}
