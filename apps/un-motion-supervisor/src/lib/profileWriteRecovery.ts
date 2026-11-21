export type ProfileWrite = {
 id: number;
 profileId: string;
 profileName: string;
 fields: Record<string, unknown>;
 state: "saving" | "failed";
 superseded: boolean;
 error: string;
 recovery: boolean;
 expectedRevision?: string;
};

// Keep the complete atomic batch. A later overlapping edit invalidates retry of
// the whole older batch; replaying only its remaining fields could change meaning.
export function beginProfileWrite(
 writes: ProfileWrite[], id: number, profileId: string, profileName: string,
 fields: Record<string, unknown>, retryId?: number, expectedRevision?: string,
): ProfileWrite[] {
 const snapshot = JSON.parse(JSON.stringify(fields)) as Record<string, unknown>;
 return [
  ...writes.filter(w => w.id !== retryId).map(w => ({...w,
   superseded: w.superseded || (w.profileId === profileId && Object.keys(w.fields).some(f => f in snapshot)),
  })),
  {id, profileId, profileName, fields: snapshot, state: "saving", superseded: false, error: "", recovery: retryId !== undefined, expectedRevision},
 ];
}

export function finishProfileWrite(writes: ProfileWrite[], id: number, error?: string): ProfileWrite[] {
 return error === undefined ? writes.filter(w => w.id !== id)
  : writes.map(w => w.id === id ? {...w, state: "failed", error} : w);
}

export function canRetryProfileWrite(write: ProfileWrite): boolean {
 return write.state === "failed" && !write.superseded && !write.error.startsWith('input_validation:');
}
