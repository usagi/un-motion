import assert from 'node:assert/strict';
import test from 'node:test';
import {ProfileSaveQueue} from '../src/lib/profileSaveQueue.ts';

test('queued same-profile edits advance only through own successful revisions', async()=>{
 const queue=new ProfileSaveQueue(), calls=[];
 const save=revision=>new Promise((resolve,reject)=>calls.push({revision,resolve,reject}));
 const first=queue.run('a','r1',save), second=queue.run('a','r1',save);
 assert.equal(calls.length,1);assert.equal(calls[0].revision,'r1');
 calls[0].resolve({revision:'r2'});await first;await Promise.resolve();
 assert.equal(calls.length,2);assert.equal(calls[1].revision,'r2');
 calls[1].resolve({revision:'r3'});await second;
 assert.equal(queue.revision('a'),'r3');
});

test('failed save cannot rebase retained input onto externally observed values',async()=>{
 const queue=new ProfileSaveQueue();
 await assert.rejects(queue.run('a','r1',async revision=>{assert.equal(revision,'r1');throw Error('profile_conflict');}));
 queue.observe('a','external');
 await assert.rejects(queue.run('a','r1',async revision=>{assert.equal(revision,'r1');throw Error('profile_conflict');}));
 await queue.run('a','external',async revision=>{assert.equal(revision,'external');return {revision:'reviewed'};});
});

test('another profile does not wait for a blocked save',async()=>{
 const queue=new ProfileSaveQueue();let finish;
 const a=queue.run('a','a1',()=>new Promise(resolve=>{finish=resolve;}));
 const b=await queue.run('b','b1',async revision=>{assert.equal(revision,'b1');return {revision:'b2'};});
 assert.equal(b.revision,'b2');finish({revision:'a2'});await a;
});

test('queued work continues after rejection without accepting a new base',async()=>{
 const queue=new ProfileSaveQueue();let reject;
 const first=queue.run('a','r1',()=>new Promise((_,fail)=>{reject=fail;}));
 const second=queue.run('a','r1',async revision=>{assert.equal(revision,'r1');return {revision:'r2'};});
 const failure=assert.rejects(first);reject(Error('offline'));await failure;await second;
});

test('legacy responses still serialize writes to the same profile', async()=>{
 const queue=new ProfileSaveQueue(), calls=[];
 const save=revision=>new Promise(resolve=>calls.push({revision,resolve}));
 const first=queue.run('a',undefined,save), second=queue.run('a',undefined,save);
 assert.equal(calls.length,1,'second edit must wait for first persistence');
 assert.equal(calls[0].revision,undefined);
 calls[0].resolve({});await first;await Promise.resolve();
 assert.equal(calls.length,2);assert.equal(calls[1].revision,undefined);
 calls[1].resolve({});await second;
});


test('captured observer rejects later read and A-B-A observation without rebasing input',()=>{
 for(const revisions of [['new'],['other','initial']]) {
  const q=new ProfileSaveQueue();q.observe('a','initial');const observe=q.captureObserver();
  for(const revision of revisions)q.observe('a',revision);
  assert.equal(observe('a','late calibration'),false);
  assert.equal(q.revision('a'),revisions.at(-1));
 }
});

test('captured observer accepts own successive results and independent profiles',()=>{
 const q=new ProfileSaveQueue();q.observe('a','a1');const observe=q.captureObserver();
 q.observe('b','b2');assert.equal(observe('a','a2'),true);assert.equal(observe('a','a3'),true);
 assert.equal(q.revision('a'),'a3');assert.equal(q.revision('b'),'b2');
});

test('a successful queued save supersedes a previously captured observer',async()=>{
 const q=new ProfileSaveQueue();q.observe('a','a1');const observe=q.captureObserver();
 await q.run('a','a1',async()=>({revision:'a2'}));
 assert.equal(observe('a','stale'),false);assert.equal(q.revision('a'),'a2');
});
