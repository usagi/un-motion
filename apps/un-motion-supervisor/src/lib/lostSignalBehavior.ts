export type LostPart = 'head' | 'hands' | 'arms';
export function resolvedLostBehavior(part: LostPart, value: string | null, fallback: string | null): 'hold' | 'drop' | 'rest-pose' {
 // An absent part uses its runtime default; an explicitly empty part inherits
 // the legacy global behavior. These two cases are deliberately different.
 const selected=value ?? (part==='head'?'hold':'rest-pose');
 const normalized=(selected.trim()===''?(fallback ?? 'rest-pose'):selected).trim().toLowerCase();
 if(normalized==='hold')return 'hold';
 if(['drop','send-none','none'].includes(normalized))return 'drop';
 return 'rest-pose';
}
