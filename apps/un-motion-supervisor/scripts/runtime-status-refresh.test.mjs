import assert from 'node:assert/strict';
import test from 'node:test';
import fs from 'node:fs';
import ts from 'typescript';
const source=fs.readFileSync(new URL('../src/App.svelte',import.meta.url),'utf8').match(/<script lang="ts">([\s\S]*?)<\/script>/)[1];
const ast=ts.createSourceFile('app.ts',source,ts.ScriptTarget.Latest,true);
const refresh=ast.statements.find(n=>ts.isFunctionDeclaration(n)&&n.name?.text==='refreshRuntimeStatuses').getText(ast);
function harness(){
 const pending=[];
 const invoke=(_command,args)=>new Promise((resolve,reject)=>pending.push({...args,resolve,reject}));
 const code=`let refreshRuntimeStatusesPromise=null,capturerListGeneration=0;
 let capturers=[{id:1,state:'running'},{id:2,state:'running'}];
 let runtimeStatuses={1:{id:1,healthy:true,fps:{zenohFramesPerSec:60}}},runtimeStatusErrors={};
 ${refresh}
 return {refresh:refreshRuntimeStatuses,state:()=>({runtimeStatuses,runtimeStatusErrors}),change:()=>{capturerListGeneration++;}};`;
 return {...new Function('invoke',ts.transpileModule(code,{compilerOptions:{target:ts.ScriptTarget.ES2022}}).outputText)(invoke),pending};
}
test('failed telemetry refresh is explicit per capturer, then successful refresh clears it',async()=>{
 const h=harness();const first=h.refresh();
 h.pending[0].reject(Error('offline'));h.pending[1].resolve({id:2,healthy:true});await first;
 assert.match(h.state().runtimeStatusErrors[1]??'',/offline/);
 assert.equal(h.state().runtimeStatusErrors[2],undefined);
 assert.equal(h.state().runtimeStatuses[2].healthy,true);
 const second=h.refresh();h.pending[2].resolve({id:1,healthy:false});h.pending[3].resolve({id:2,healthy:true});await second;
 assert.equal(h.state().runtimeStatusErrors[1],undefined);
 assert.equal(h.state().runtimeStatuses[1].healthy,false);
});
test('a response from an obsolete capturer list cannot set errors or metrics',async()=>{
 const h=harness();const first=h.refresh();h.change();
 h.pending[0].reject(Error('obsolete'));h.pending[1].resolve({id:2,healthy:true});await first;
 assert.deepEqual(h.state().runtimeStatusErrors,{});
 assert.equal(h.state().runtimeStatuses[2],undefined);
});
