import assert from 'node:assert/strict';
import test from 'node:test';
import {readFile} from 'node:fs/promises';
import ts from 'typescript';
const newProfileDefaults=JSON.parse(await readFile(new URL('../src/lib/newProfileDefaults.json',import.meta.url),'utf8'));
const text=await readFile(new URL('../src/dev-ipc-mock.ts',import.meta.url),'utf8');
function harness(){
 const source=text.replace(/^import .*;\r?\n/gm,'').replace('export function installDevIpcMock','function installDevIpcMock');
 const code=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.None}}).outputText;
 let ipc;
 new Function('mockIPC','enLocaleToml','jaLocaleToml','sessionStorage','newProfileDefaults',code+'\ninstallDevIpcMock();')(handler=>ipc=handler,'','',{getItem:()=>null,setItem:()=>{}},newProfileDefaults);
 return (cmd,args={})=>ipc(cmd,args);
}
test('preview delete/restore preserves full profile and other profile',()=>{
 const invoke=harness();const all=invoke('list_profiles');const id=all[0].id, other=all[1].id;
 const original=structuredClone(invoke('get_profile_detail',{profileId:id}));
 const beforeOther=structuredClone(invoke('get_profile_detail',{profileId:other}));
 invoke('set_selected_profile_id',{profileId:other});
 invoke('delete_profile',{profileId:id});
 assert.throws(()=>invoke('get_profile_detail',{profileId:id}));
 const archive=invoke('list_deleted_profiles')[0];
 invoke('restore_deleted_profile',{archiveId:archive.archive_id});
 assert.deepEqual(invoke('get_profile_detail',{profileId:id}),original);
 assert.deepEqual(invoke('get_profile_detail',{profileId:other}),beforeOther);
 assert.equal(invoke('selected_profile_id'),other);
 assert.equal(invoke('list_deleted_profiles').length,1);
 assert.throws(()=>invoke('restore_deleted_profile',{archiveId:archive.archive_id}),/already exists/);
});
test('preview deletion rejects running and missing targets without archiving',()=>{
 const invoke=harness();const id=invoke('list_profiles')[0].id;
 invoke('launch_capturer',{profileId:id});
 assert.throws(()=>invoke('delete_profile',{profileId:id}),/in use/);
 assert.throws(()=>invoke('delete_profile',{profileId:'missing'}),/not found/);
 assert.deepEqual(invoke('list_deleted_profiles'),[]);
});
test('preview archive is immutable across subsequent edits and restores empty default',()=>{
 const invoke=harness();const all=invoke('list_profiles'),id=all[0].id;
 const original=structuredClone(invoke('get_profile_detail',{profileId:id}));
 for(const p of all) invoke('delete_profile',{profileId:p.id});
 const archive=invoke('list_deleted_profiles')[0];
 assert.equal(invoke('selected_profile_id'),'');
 assert.equal(invoke('restore_deleted_profile',{archiveId:archive.archive_id}).selectedProfileId,id);
 invoke('get_profile_detail',{profileId:id}).note='changed after restoration';
 invoke('delete_profile',{profileId:id});
 invoke('restore_deleted_profile',{archiveId:archive.archive_id});
 assert.deepEqual(invoke('get_profile_detail',{profileId:id}),original);
 assert.throws(()=>invoke('restore_deleted_profile',{archiveId:'missing'}),/not found/);
});

test('preview duplicate can be edited, deleted and restored without changing original or default',()=>{
 const invoke=harness();const id=invoke('list_profiles')[0].id;
 const original=structuredClone(invoke('get_profile_detail',{profileId:id}));
 const selected=invoke('selected_profile_id');
 const copy=invoke('duplicate_profile',{profileId:id});
 assert.notEqual(copy.id,id);assert.deepEqual(copy.runtime,original.runtime);assert.deepEqual(copy.pipeline,original.pipeline);
 const another=invoke('duplicate_profile',{profileId:id});assert.notEqual(another.id,copy.id);
 invoke('update_profile_field',{profileId:copy.id,field:'note',value:'Independent copy'});
 invoke('update_profile_field',{profileId:copy.id,field:'runtime_selection.zenoh_lan_enabled',value:true});
 const edited=structuredClone(invoke('get_profile_detail',{profileId:copy.id}));
 invoke('delete_profile',{profileId:copy.id});
 const archive=invoke('list_deleted_profiles').find(a=>a.profile_id===copy.id);
 invoke('restore_deleted_profile',{archiveId:archive.archive_id});
 assert.deepEqual(invoke('get_profile_detail',{profileId:copy.id}),edited);
 assert.deepEqual(invoke('get_profile_detail',{profileId:id}),original);
 assert.equal(invoke('selected_profile_id'),selected);
 assert.throws(()=>invoke('duplicate_profile',{profileId:'missing'}),/not found/);
});

test('preview batch validates every field before committing any change',()=>{
 const invoke=harness();const id=invoke('list_profiles')[0].id;
 const before=structuredClone(invoke('get_profile_detail',{profileId:id}));
 for(const update of [{field:'unknown',value:true},{field:'runtime_selection.zenoh_lan_port',value:70000},{field:'runtime_selection.zenoh_lan_enabled',value:'false'}]){
  assert.throws(()=>invoke('update_profile_fields',{profileId:id,updates:[{field:'note',value:'must not persist'},update]}));
  assert.deepEqual(invoke('get_profile_detail',{profileId:id}),before);
 }
 const result=invoke('update_profile_fields',{profileId:id,updates:[{field:'note',value:'saved'},{field:'runtime_selection.zenoh_lan_enabled',value:false},{field:'runtime_selection.zenoh_lan_port',value:47448}]});
 assert.equal(result.note,'saved');assert.equal(result.runtime.zenohLanConnectAddress,null);
 result.note='client mutated';assert.equal(invoke('get_profile_detail',{profileId:id}).note,'saved');
});

test('preview direct pipeline fields match backend parser inventory and reject partial invalid batches',async()=>{
 const rust=await readFile(new URL('../src-tauri/src/lib.rs',import.meta.url),'utf8');
 const block=rust.slice(rust.indexOf('fn apply_pipeline_field('),rust.indexOf('fn runtime_is_empty('));
 const fields=[...block.matchAll(/"([a-z_]+)" => pipeline\.[a-z_]+ = (opt_\w+)\(&value, field\)/g)].filter(m=>!m[1].includes('denoise'));
 assert.equal(fields.length,16);
 const invoke=harness();const id=invoke('list_profiles')[0].id;
 for(const [,field,parser] of fields){
  const key=field.replace(/_([a-z])/g,(_,c)=>c.toUpperCase());
  const value=parser==='opt_string'?' sample ':parser==='opt_bool'?true:320;
  const result=invoke('update_profile_fields',{profileId:id,updates:[{field:`pipeline_components.${field}`,value}]});
  assert.equal(result.pipeline[key],typeof value==='string'?value.trim():value,field);
  const before=structuredClone(invoke('get_profile_detail',{profileId:id}));
  assert.throws(()=>invoke('update_profile_fields',{profileId:id,updates:[{field:'note',value:'invalid batch'}, {field:`pipeline_components.${field}`,value:[]}]}));
  assert.deepEqual(invoke('get_profile_detail',{profileId:id}),before);
  assert.equal(invoke('update_profile_field',{profileId:id,field:`pipeline_components.${field}`,value:null}).pipeline[key],null);
 }
 assert.throws(()=>invoke('update_profile_field',{profileId:id,field:'runtime_selection.zenoh_lan_port',value:0}));
});

test('preview runtime settings retain types, normalize thread minimum and reject partial failure',()=>{
 const invoke=harness();const all=invoke('list_profiles'),id=all[0].id,other=all[1].id;
 const beforeOther=structuredClone(invoke('get_profile_detail',{profileId:other}));
 const updates=[['fps',90],['vmc_enabled',false],['vmc_target_addr',' 127.0.0.1:39540 '],['device',' camera-b '],['media_pipe_num_threads',0],['media_pipe_holistic_flow_limiter_max_in_flight',0],['media_pipe_holistic_flow_limiter_max_in_queue',0],['vrc_osc_process_poll_interval_secs',20]].map(([field,value])=>({field:`runtime_selection.${field}`,value}));
 const result=invoke('update_profile_fields',{profileId:id,updates});
 assert.equal(result.runtime.fps,90);assert.equal(result.runtime.vmcEnabled,false);assert.equal(result.runtime.vmcTargetAddr,'127.0.0.1:39540');
 assert.equal(result.runtime.device,'camera-b');assert.equal(result.runtime.mediaPipeNumThreads,1);
 assert.equal(result.runtime.mediaPipeHolisticFlowLimiterMaxInFlight,1);assert.equal(result.runtime.mediaPipeHolisticFlowLimiterMaxInQueue,0);
 assert.deepEqual(invoke('get_profile_detail',{profileId:other}),beforeOther);
 const before=structuredClone(invoke('get_profile_detail',{profileId:id}));
 for(const [field,value] of [['fps',-1],['fps',4294967296],['vmc_enabled','true'],['vrc_osc_process_poll_interval_secs',Number.MAX_SAFE_INTEGER+1]]){
  assert.throws(()=>invoke('update_profile_fields',{profileId:id,updates:[{field:'note',value:'must not save'},{field:`runtime_selection.${field}`,value}]}));
  assert.deepEqual(invoke('get_profile_detail',{profileId:id}),before);
 }
 assert.equal(invoke('update_profile_field',{profileId:id,field:'runtime_selection.device',value:'  '}).runtime.device,null);
 assert.equal(invoke('update_profile_field',{profileId:id,field:'runtime_selection.fps',value:null}).runtime.fps,null);
});

test('preview modifier and signal loss fields match native field inventory and numeric rules',async()=>{
 const rust=await readFile(new URL('../src-tauri/src/lib.rs',import.meta.url),'utf8');
 const invoke=harness();const id=invoke('list_profiles')[0].id;let count=0;
 for(const [fn,prefix] of [['apply_modifier_field','runtime_selection.modifier.'],['apply_post_process_rules_field','runtime_selection.modifier.post_process_rules.']]){
  const start=rust.indexOf(`fn ${fn}(`);const block=rust.slice(start,rust.indexOf('\nfn ',start+1));
  for(const [,field,parser] of block.matchAll(/"([a-z0-9_]+)" => \w+\.[a-z0-9_]+ = (opt_\w+)\(&value, field\)/g)){
   count++;const path=prefix+field,key=('modifier_'+field).replace(/_([a-z])/g,(_,c)=>c.toUpperCase());
   const value=parser==='opt_bool'?true:parser==='opt_string'?' hold ':0.3;
   const result=invoke('update_profile_field',{profileId:id,field:path,value});
   assert.equal(result.runtime[key],parser==='opt_f32'?Math.fround(value):parser==='opt_string'?'hold':value,path);
   assert.equal(invoke('update_profile_field',{profileId:id,field:path,value:null}).runtime[key],null,path);
  }
 }
 assert.equal(count,37);
 const clamped=invoke('update_profile_fields',{profileId:id,updates:[{field:'runtime_selection.modifier.eye_open_bias',value:2},{field:'runtime_selection.modifier.torso_pitch_scale',value:2}]});
 assert.equal(clamped.runtime.modifierEyeOpenBias,1);assert.equal(clamped.runtime.modifierTorsoPitchScale,1);
 const before=structuredClone(invoke('get_profile_detail',{profileId:id}));
 for(const value of [-0.1,Infinity,NaN,3.5e38]){
  assert.throws(()=>invoke('update_profile_fields',{profileId:id,updates:[{field:'note',value:'rejected'},{field:'runtime_selection.modifier.adaptive_beta',value}]}));
  assert.deepEqual(invoke('get_profile_detail',{profileId:id}),before);
 }
});

test('preview legacy denoise retains raw mode across display normalization and archive restoration',()=>{
 const invoke=harness();const id=invoke('list_profiles')[0].id;
 let result=invoke('update_profile_field',{profileId:id,field:'pipeline_components.input_denoise_mode',value:' TEMPORAL_IIR_8HZ '});
 assert.equal(result.pipeline.inputDenoiseMode,'temporal-iir');assert.equal(result.pipeline.inputDenoiseTemporalIirHz,8);
 result=invoke('update_profile_field',{profileId:id,field:'pipeline_components.input_denoise_temporal_iir_hz',value:3});assert.equal(result.pipeline.inputDenoiseTemporalIirHz,3);
 invoke('delete_profile',{profileId:id});const archived=invoke('list_deleted_profiles')[0];invoke('restore_deleted_profile',{archiveId:archived.archive_id});
 result=invoke('update_profile_field',{profileId:id,field:'pipeline_components.input_denoise_temporal_iir_hz',value:null});assert.equal(result.pipeline.inputDenoiseTemporalIirHz,8);
 result=invoke('update_profile_field',{profileId:id,field:'pipeline_components.input_denoise_mode',value:null});assert.equal(result.pipeline.inputDenoiseMode,'off');assert.equal(result.pipeline.inputDenoiseTemporalIirHz,null);
});
test('preview engine aliases and icon clearing follow saved profile normalization',()=>{
 const invoke=harness();const id=invoke('list_profiles')[0].id;
 assert.equal(invoke('update_profile_field',{profileId:id,field:'runtime_selection.engine',value:' media-pipe-native '}).runtime.engine,'mediapipe-native');
 assert.equal(invoke('update_profile_field',{profileId:id,field:'runtime_selection.engine',value:'invalid'}).runtime.engine,'mediapipe-native');
 assert.equal(invoke('update_profile_field',{profileId:id,field:'icon_path',value:' icon.webp '}).iconPath,'icon.webp');
 assert.equal(invoke('update_profile_field',{profileId:id,field:'icon_path',value:' '}).iconPath,null);
});

test('preview face model preserves hidden metrics across archive and disabling removes entire model',()=>{
 const invoke=harness();const id=invoke('list_profiles')[0].id,prefix='runtime_selection.modifier.face_pose_model.';
 const fields={enabled:true,neutral_nose_drop_eye_mouth:0.3,sample_count:90,median_abs_yaw:0.2,median_abs_roll:0.1};
 const result=invoke('update_profile_fields',{profileId:id,updates:Object.entries(fields).map(([key,value])=>({field:prefix+key,value}))});
 assert.equal(result.runtime.modifierFacePoseModelEnabled,true);assert.equal(result.runtime.modifierFacePoseModelSampleCount,90);
 assert.equal(result.previewFacePoseModel[prefix+'median_abs_yaw'],Math.fround(0.2));
 assert.equal(result.runtime.modifierFacePoseModelMedianAbsYaw,undefined);
 invoke('delete_profile',{profileId:id});invoke('restore_deleted_profile',{archiveId:invoke('list_deleted_profiles')[0].archive_id});
 assert.deepEqual(invoke('get_profile_detail',{profileId:id}).previewFacePoseModel,result.previewFacePoseModel);
 const disabled=invoke('update_profile_field',{profileId:id,field:prefix+'enabled',value:false});
 assert.deepEqual(disabled.previewFacePoseModel,{});assert.equal(disabled.runtime.modifierFacePoseModelEnabled,null);assert.equal(disabled.runtime.modifierFacePoseModelSampleCount,null);
 const reenabled=invoke('update_profile_field',{profileId:id,field:prefix+'enabled',value:true});assert.equal(reenabled.runtime.modifierFacePoseModelSampleCount,null);
 const before=structuredClone(invoke('get_profile_detail',{profileId:id}));
 assert.throws(()=>invoke('update_profile_fields',{profileId:id,updates:[{field:prefix+'enabled',value:false},{field:prefix+'sample_count',value:-1}]}));
 assert.deepEqual(invoke('get_profile_detail',{profileId:id}),before);
});


test('preview creates native defaults independently and supports empty-library creation',()=>{
 const invoke=harness();const first=invoke('create_profile',{name:''});
 assert.equal(first.name,'New Profile');assert.deepEqual(first.runtime,newProfileDefaults.runtime);assert.deepEqual(first.pipeline,newProfileDefaults.pipeline);
 assert.equal(invoke('selected_profile_id'),first.id);
 invoke('update_profile_field',{profileId:first.id,field:'runtime_selection.fps',value:120});
 const second=invoke('create_profile',{name:''});assert.notEqual(second.id,first.id);assert.deepEqual(second.runtime,newProfileDefaults.runtime);
 for(const profile of invoke('list_profiles'))invoke('delete_profile',{profileId:profile.id});
 const created=invoke('create_profile',{name:'日本語'});assert.equal(created.id,'profile');assert.equal(created.name,'日本語');
 assert.equal(invoke('selected_profile_id'),created.id);assert.deepEqual(created.runtime,newProfileDefaults.runtime);
});

test('all 91 preview field updates match native normalized display contracts',async()=>{
 const cases=JSON.parse(await readFile(new URL('./preview-save-native-contract.json',import.meta.url),'utf8'));
 assert.equal(cases.length,91);const differences=[];
 function compare(expected,actual,path,field){
  if(expected && typeof expected==='object') {for(const [key,value] of Object.entries(expected))compare(value,actual?.[key],`${path}.${key}`,field);return;}
  if(typeof expected==='number' && typeof actual==='number' && Math.abs(expected-actual)<=Number.EPSILON*Math.max(1,Math.abs(expected))*2)return;
  if(expected!==actual)differences.push({field,path,expected,actual});
 }
 for(const {input,expected} of cases){const invoke=harness();const profile=invoke('create_profile',{name:''});const result=invoke('update_profile_field',{profileId:profile.id,...input});compare(expected,result,'profile',input.field);}
 assert.deepEqual(differences,[]);
});
