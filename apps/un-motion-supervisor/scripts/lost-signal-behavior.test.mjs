import test from 'node:test';import assert from 'node:assert/strict';
import {resolvedLostBehavior} from '../src/lib/lostSignalBehavior.ts';
test('absent part retains native default while empty part inherits global setting',()=>{
 assert.equal(resolvedLostBehavior('head',null,'drop'),'hold');
 assert.equal(resolvedLostBehavior('head',' ','drop'),'drop');
 assert.equal(resolvedLostBehavior('hands',null,'hold'),'rest-pose');
 assert.equal(resolvedLostBehavior('hands','','hold'),'hold');
});
test('native aliases and case normalize without changing saved strings',()=>{
 for(const value of ['drop',' NONE ','SEND-NONE'])assert.equal(resolvedLostBehavior('arms',value,null),'drop');
 assert.equal(resolvedLostBehavior('head',' HOLD ',null),'hold');
 assert.equal(resolvedLostBehavior('head','unknown','hold'),'rest-pose');
 assert.equal(resolvedLostBehavior('head','',null),'rest-pose');
});
