# Lists English strings used in the code that have no translations yet.
# Run from the repository root: python3 i18n/missing.py
import re,glob,sys
src="".join(open(f).read() for f in glob.glob('src/**/*.rs',recursive=True) if 'i18n_table' not in f)
table=open('src/i18n_table.rs').read()
keys=set()
for m in re.finditer(r'\(\s*"((?:[^"\\]|\\.)*)",\s*"((?:[^"\\]|\\.)*)"\),', table):
    keys.add(m.group(1))
found=set()
for m in re.finditer(r'\btr\(\s*"((?:[^"\\]|\\.)*)"', src): found.add(m.group(1))
for m in re.finditer(r'\b(?:row|header)\(\s*"((?:[^"\\]|\\.)*)"(?:\s*,\s*"((?:[^"\\]|\\.)*)")?', src):
    found.add(m.group(1))
    if m.group(2): found.add(m.group(2))
for m in re.finditer(r'tr\(if [^{]*\{\s*"((?:[^"\\]|\\.)*)"\s*\}\s*else\s*\{\s*"((?:[^"\\]|\\.)*)"', src):
    found.add(m.group(1)); found.add(m.group(2))
miss=sorted(x for x in found if x not in keys and x.strip())
for x in miss: print(repr(x))
