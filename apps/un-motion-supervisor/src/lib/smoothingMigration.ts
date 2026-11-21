type Runtime = {
 modifierSmoothingPreset: string | null;
 modifierSmoothingEmaEnabled: boolean | null;
 modifierSmoothingOneEuroEnabled: boolean | null;
 modifierSmoothingEmaAlpha: number | null;
};
const prefix = 'runtime_selection.modifier.';
export function effectiveSmoothing(r: Runtime) {
 // The runtime prefers explicit enabled stages; only otherwise uses the legacy preset.
 if (r.modifierSmoothingEmaEnabled || r.modifierSmoothingOneEuroEnabled)
  return {ema:!!r.modifierSmoothingEmaEnabled, oneEuro:!!r.modifierSmoothingOneEuroEnabled, alpha:r.modifierSmoothingEmaAlpha ?? .45};
 const preset=r.modifierSmoothingPreset?.toLowerCase();
 return {ema:['low','medium','high'].includes(preset ?? ''),oneEuro:preset==='adaptive',alpha:preset==='low'?.7:preset==='high'?.25:preset==='medium'?.45:(r.modifierSmoothingEmaAlpha ?? .45)};
}
export function smoothingMigration(r: Runtime, fields: Record<string,unknown>, pending: Record<string,unknown>[] = []): Record<string,unknown> {
 const relevant=Object.keys(fields).some(f=>f.startsWith(prefix+'smoothing_')||f.startsWith(prefix+'adaptive_'));
 if(!relevant)return fields;
 const preset=r.modifierSmoothingPreset?.toLowerCase();
 if(!relevant||!['low','medium','high','adaptive'].includes(preset ?? '')||prefix+'smoothing_preset' in fields)return fields;
 const active=effectiveSmoothing(r);
 // A previous migration may still be saving while the visible runtime is old.
 // Retain that user's edits instead of reconstructing the legacy defaults again.
 const previous=Object.assign({}, ...pending.map(batch=>Object.fromEntries(Object.entries(batch).filter(([field])=>field.startsWith(prefix+'smoothing_')||field.startsWith(prefix+'adaptive_')))));
 return {[prefix+'smoothing_preset']:'off',[prefix+'smoothing_ema_enabled']:active.ema,[prefix+'smoothing_one_euro_enabled']:active.oneEuro,[prefix+'smoothing_ema_alpha']:active.alpha,...previous,...fields};
}
