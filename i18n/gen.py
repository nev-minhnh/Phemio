import json, os, sys
# Builds src/i18n_table.rs from i18n/keys.json and i18n/tr_<lang>.json.
# Run from anywhere: python3 i18n/gen.py
SP=os.path.dirname(os.path.abspath(__file__))
d=json.load(open(f'{SP}/keys.json'))
keys=d['keys']
dead=set(json.load(open(f'{SP}/dead_keys.json'))) if os.path.exists(f'{SP}/dead_keys.json') else set()
keys=[k for k in keys if k not in dead]
def esc(x):
    return x.replace('\\','\\\\').replace('"','\\"').replace('\n','\\n').replace('\t','\\t')
out=['// Generated: English → each language, sorted for binary search.','// (Missing entries simply show the English.)','']
for code in ['vi','zh','ko','ja','fr','es']:
    path=f'{SP}/tr_{code}.json'
    tr=json.load(open(path)) if os.path.exists(path) else ({k:v for k,v in d['vi'].items()} if code=='vi' else {})
    if code=='vi':
        # keep any old entries the new file lacks
        for k,v in d['vi'].items(): tr.setdefault(k,v)
    items=sorted(((k,v) for k,v in tr.items() if (k in keys or code=='vi') and k not in dead), key=lambda kv: kv[0].encode('utf-8'))
    out.append(f'static TABLE_{code.upper()}: &[(&str, &str)] = &[')
    for k,v in items:
        if not v.strip(): continue
        out.append(f'    ("{esc(k)}", "{esc(v)}"),')
    out.append('];')
    out.append('')
open(os.path.join(SP,'..','src','i18n_table.rs'),'w',encoding='utf-8').write('\n'.join(out))
print('ok', {c: os.path.exists(f'{SP}/tr_{c}.json') for c in ['vi','zh','ko','ja','fr','es']})
