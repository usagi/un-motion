// Serialize revision-aware writes per profile. Only our own successful saves
// advance a queued edit's base; observing external data never rebases old input.
export class ProfileSaveQueue {
 private tails = new Map<string, Promise<unknown>>();
 private observed = new Map<string, string>();
 private observationVersions = new Map<string, number>();
 private chains = new Map<string, { current: string; ancestors: Set<string> }>();

 observe(id: string, revision?: string) {
  if (!revision) return;
  this.observed.set(id, revision);
  this.observationVersions.set(id, (this.observationVersions.get(id) ?? 0) + 1);
 }
 revision(id: string) { return this.observed.get(id); }

 // An asynchronous external operation may report a result older than a later
 // read or save. It must not move the observed revision backwards, even A-B-A.
 captureObserver() {
  const versions = new Map(this.observationVersions);
  return (id: string, revision?: string) => {
   if ((versions.get(id) ?? 0) !== (this.observationVersions.get(id) ?? 0)) return false;
   this.observe(id, revision);
   versions.set(id, this.observationVersions.get(id) ?? 0);
   return true;
  };
 }

 run<T extends {revision?: string}>(id: string, base: string | undefined, save: (expected: string | undefined) => Promise<T>): Promise<T> {
  const execute = async () => {
   // Older hosts omit revisions, but their writes still need the same ordering.
   if (!base) return save(undefined);
   const chain = this.chains.get(id);
   const expected = chain?.ancestors.has(base) ? chain.current : base;
   const result = await save(expected);
   if (result.revision) {
    const ancestors = chain?.current === expected ? new Set(chain.ancestors) : new Set<string>();
    ancestors.add(expected);
    ancestors.add(result.revision);
    this.chains.set(id, {current: result.revision, ancestors});
    this.observe(id, result.revision);
   }
   return result;
  };
  const previous = this.tails.get(id);
  const result = previous ? previous.catch(() => undefined).then(execute) : execute();
  this.tails.set(id, result);
  const clear = () => { if (this.tails.get(id) === result) this.tails.delete(id); };
  void result.then(clear, clear);
  return result;
 }
}
